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

/// Near and far planes of the eye projections (metres in the room): the HAL's one definition.
use makepad_hal::{XR_EYE_FAR_M as EYE_FAR, XR_EYE_NEAR_M as EYE_NEAR};

/// The app uses the room field (`Cx::xr_set_room_field`): the depth readback is integrated.
pub(crate) static ROOM_FIELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) struct HalXr {
    /// the Cx window standing for the HAL's XR window
    pub(crate) window: WindowId,
    /// the newest `XrFrame`: the eyes of the next frame
    pub(crate) frame: Option<XrFrame>,
    /// its predicted display time (HAL clock, ns)
    pub(crate) display_ns: u64,
    /// The frames' clock to this process's (ns, added): 0 while their display times are on its
    /// clock (a headset of its own); a remote headset's are on the headset's clock, before this
    /// process started (its `XrState::time` stayed 0): the first such frame maps to now.
    pub(crate) clock_offset_ns: i64,
    pub(crate) last: Rc<XrState>,
    /// the pass uniforms of the frame being built, one per eye (alive until the batch is sent);
    /// a multiview pass uses the first, with both eyes in it
    pub(crate) eye_uniforms: [DrawPassUniforms; 2],
    /// the TSDF depth writer the headset's environment depth goes to (`cx.xr_tsdf()`), made
    /// at the first depth frame
    pub(crate) depth_writer: Option<makepad_tsdf::DepthWriter>,
    /// The newest environment depth image as a texture (the runtime's image itself: both eyes'
    /// layers, depth 0..1 in .r) and its header (`Cx::xr_environment_depth`).
    pub(crate) env_depth: Option<(crate::texture::Texture, XrDepth)>,
    /// when the hand depth self-check last logged
    pub(crate) hand_check_at: Option<std::time::Instant>,
    /// the runtime's depth images imported as textures (by their `XrDepth::image` ticket)
    pub(crate) env_images: Vec<(u64, crate::texture::Texture)>,
    /// the display time of the frame that acquired the current `env_depth` image
    pub(crate) env_depth_ns: u64,
    /// `Cx::xr_set_scene_to_room`: the XR passes' scene space into the room (tracking space);
    /// the eye views of the pass uniforms are composed with it (identity: the scene is the room)
    pub(crate) scene_to_room: Mat4f,
    /// `Cx::xr_set_supersample`: an offscreen XR pass (`info_flags::XR_OFFSCREEN`) renders at
    /// this many of its eye pixels per axis (1: the eye size); the app filters it down itself
    pub(crate) supersample: f32,
    /// `Cx::xr_set_msaa`: the XR passes' samples per pixel (1: none)
    pub(crate) msaa: u32,
}

/// A local-space point seen by eye 0's environment depth camera: its depth along that camera's
/// axis, and the image's depth at its texel (`None`: no depth there); `None` outside the image.
fn env_depth_at(header: &XrDepth, texels: &[u16], p: [f32; 3]) -> Option<(f32, Option<f32>)> {
    let v = &header.views[0];
    let [x, y, z, w] = v.pose.orientation;
    let q = Quat { x: -x, y: -y, z: -z, w };
    let d = Vec3f { x: p[0] - v.pose.position[0], y: p[1] - v.pose.position[1], z: p[2] - v.pose.position[2] };
    let e = q.rotate_vec3(&d);
    let ze = -e.z;
    if ze <= 0.01 { return None; }
    let [l, r, u, dn] = v.fov.map(f32::tan);
    let (uu, vv) = ((e.x / ze - l) / (r - l), (e.y / ze - dn) / (u - dn));
    if !(0.0..1.0).contains(&uu) || !(0.0..1.0).contains(&vv) { return None; }
    let (tx, ty) = ((uu * header.width as f32) as i32, (vv * header.height as f32) as i32);
    // the nearest depth within 12 texels (about 3 degrees: a hand that moved since the capture
    // is still found)
    let (w, h) = (header.width as i32, header.height as i32);
    let mut raw = 1.0f32;
    for y in (ty - 12).max(0)..(ty + 13).min(h) {
        for x in (tx - 12).max(0)..(tx + 13).min(w) {
            let d = *texels.get((y * w + x) as usize)? as f32 / 65535.0;
            if d > 0.0 { raw = raw.min(d); }
        }
    }
    let n = header.near;
    let image = (raw > 0.0 && raw < 0.99999).then(|| if header.far > 0.0 { n * header.far / (header.far - raw * (header.far - n)) } else { n / (1.0 - raw) });
    Some((ze, image))
}

// asked before the XR window exists: applied when it is made
thread_local! { static SEE_THROUGH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
thread_local! { static SUPERSAMPLE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) }; }
thread_local! { static MSAA: std::cell::Cell<u32> = const { std::cell::Cell::new(1) }; }
thread_local! { static REFRESH_MHZ: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }

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
    /// The HAL's info flags as they are now: a remote headset slot is offered from the moment a
    /// headset links (`makepad_hal::xr_remote::hal_xr_remote_enable`), after the HAL opened.
    pub(crate) fn hal_info_flags(&self) -> u32 {
        let Some(info) = self.hal.info else { return 0 };
        let remote = if makepad_hal::xr_remote::hal_xr_remote_enabled() { makepad_hal::info_flags::XR | makepad_hal::info_flags::XR_OFFSCREEN } else { 0 };
        info.flags | remote
    }

    /// `xr_start_presenting`: the XR window, once (a backend without XR: nothing to present on).
    pub(crate) fn hal_xr_start(&mut self) {
        if self.hal.xr.is_some() { return; }
        if self.hal_info_flags() & makepad_hal::info_flags::XR == 0 {
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
        self.hal.xr = Some(HalXr { window, frame: None, display_ns: 0, clock_offset_ns: 0, last: Rc::new(XrState::default()), eye_uniforms: Default::default(), depth_writer: None, env_depth: None, hand_check_at: None, env_images: Vec::new(), env_depth_ns: 0, scene_to_room: Mat4f::identity(), supersample: SUPERSAMPLE.with(|s| s.get()), msaa: MSAA.with(|m| m.get()) });
        if SEE_THROUGH.with(|s| s.get()) { self.hal_xr_see_through(true); }
        let refresh = REFRESH_MHZ.with(|r| r.get());
        if refresh != 0 { self.hal_xr_refresh_rate(refresh); }
        // the XR passes paint into it from now on
        self.redraw_all();
    }

    pub(crate) fn hal_xr_see_through_now(&self) -> bool {
        SEE_THROUGH.with(|s| s.get())
    }

    pub(crate) fn hal_xr_see_through(&mut self, on: bool) {
        SEE_THROUGH.with(|s| s.set(on));
        let Some(xr) = self.hal.xr.as_ref() else { return };
        self.hal.frames.w.window_visuals(hal_window_id(xr.window), on as u32, [0.0; 4]);
    }

    /// `Cx::xr_haptic`: the HAL plays it on the XR window's controller, a headset of this
    /// process's own. A remote headset's window (offscreen XR: the stream server sends the
    /// pulse down its link) is not told.
    pub(crate) fn hal_xr_haptic(&mut self, pulse: crate::event::xr::XrHapticPulse) {
        if self.hal_info_flags() & makepad_hal::info_flags::XR_OFFSCREEN != 0 {
            return;
        }
        let Some(xr) = self.hal.xr.as_ref() else { return };
        self.hal.frames.w.xr_haptic(hal_window_id(xr.window), pulse.hand as u32, pulse.amplitude, (pulse.seconds * 1e6) as u32);
    }

    /// `Cx::xr_present_eyes`: the eyes of the XR window's next frame.
    pub(crate) fn hal_xr_present_eyes(&mut self, eyes: [makepad_hal::XrEye; 2], focus_m: f32) {
        let Some(xr) = self.hal.xr.as_ref() else { return };
        let mut bytes = Vec::with_capacity(2 * <makepad_hal::XrEye as makepad_hal::buffers::Wire>::SIZE + 4);
        for e in &eyes { makepad_hal::buffers::Wire::write(e, &mut bytes); }
        bytes.extend_from_slice(&focus_m.to_le_bytes());
        self.hal.frames.w.xr_frame_eyes(hal_window_id(xr.window), &bytes);
    }

    /// `Cx::xr_set_supersample`: the offscreen XR pass repaints at its new size.
    pub(crate) fn hal_xr_supersample(&mut self, scale: f32) {
        let scale = if scale.is_finite() { scale.clamp(0.5, 2.0) } else { 1.0 };
        SUPERSAMPLE.with(|s| s.set(scale));
        let Some(xr) = self.hal.xr.as_mut() else { return };
        if xr.supersample == scale { return; }
        xr.supersample = scale;
        let passes: Vec<_> = self.passes.id_iter().filter(|p| matches!(self.passes[*p].parent, CxDrawPassParent::Xr)).collect();
        for pass in passes { self.redraw_pass_and_child_passes(pass); }
    }

    /// `Cx::xr_set_msaa`: the XR passes repaint with that many samples per pixel (a count
    /// the HAL has, `HalInfo::msaa`; else 1).
    pub(crate) fn hal_xr_msaa(&mut self, samples: u32) {
        let supported = self.hal.info.map_or(1, |i| i.msaa);
        // the most samples the GPU has up to the count asked for
        let samples = [8u32, 4, 2].into_iter().find(|n| *n <= samples && supported & n != 0).unwrap_or(1);
        MSAA.with(|m| m.set(samples));
        let Some(xr) = self.hal.xr.as_mut() else { return };
        if xr.msaa == samples { return; }
        xr.msaa = samples;
        let passes: Vec<_> = self.passes.id_iter().filter(|p| matches!(self.passes[*p].parent, CxDrawPassParent::Xr)).collect();
        for pass in passes { self.redraw_pass_and_child_passes(pass); }
    }

    /// The XR passes' samples per pixel (`Cx::xr_set_msaa`).
    pub(crate) fn hal_xr_msaa_now(&self) -> u32 {
        self.hal.xr.as_ref().map_or(1, |xr| xr.msaa)
    }

    /// The offscreen XR pass's pixels per eye pixel (`Cx::xr_set_supersample`).
    pub(crate) fn hal_xr_supersample_now(&self) -> f32 {
        self.hal.xr.as_ref().map_or(1.0, |xr| xr.supersample)
    }

    /// `Cx::xr_set_pose_lead`.
    pub(crate) fn hal_xr_pose_lead(&mut self, lead_ns: u64) {
        let Some(xr) = self.hal.xr.as_ref() else { return };
        self.hal.frames.w.xr_pose_lead(hal_window_id(xr.window), lead_ns);
    }

    /// `Cx::xr_set_refresh_rate`.
    pub(crate) fn hal_xr_refresh_rate(&mut self, millihertz: u32) {
        REFRESH_MHZ.with(|r| r.set(millihertz));
        let Some(xr) = self.hal.xr.as_ref() else { return };
        self.hal.frames.w.xr_refresh_rate(hal_window_id(xr.window), millihertz);
    }

    pub(crate) fn hal_xr_stop(&mut self) {
        let Some(xr) = self.hal.xr.take() else { return };
        self.hal.frames.w.window_close(hal_window_id(xr.window));
        self.hal.windows.remove(&xr.window.0);
    }

    /// An `XrFrame`: the eyes of the XR window's next frame, and `Event::XrUpdate` for the app.
    pub(crate) fn hal_xr_frame(&mut self, display_ns: u64, bytes: &[u8]) {
        let Some(frame) = read_items::<XrFrame>(bytes, 1).next() else { return };
        let now_ns = makepad_hal::clock_ns();
        let start_ns = self.hal.start_ns;
        let Some(xr) = self.hal.xr.as_mut() else { return };
        // (a display time more than 10 s off this process's clock is another clock's: from the
        // first one, on this clock by the offset then)
        if xr.clock_offset_ns == 0 && (display_ns as i64 - now_ns as i64).abs() > 10_000_000_000 {
            xr.clock_offset_ns = now_ns as i64 - display_ns as i64;
        }
        let local_ns = (display_ns as i64 + xr.clock_offset_ns).max(0) as u64;
        let time = local_ns.saturating_sub(start_ns) as f64 / 1e9;
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
        // (what is redrawn from here is for the headset's frame: it paints no window)
        let in_xr = std::mem::replace(&mut self.in_xr_update, true);
        let passes: Vec<_> = self.passes.id_iter().filter(|p| matches!(self.passes[*p].parent, CxDrawPassParent::Xr)).collect();
        for pass in passes { self.redraw_pass_and_child_passes(pass); }
        self.call_event_handler(&Event::XrUpdate(XrUpdateEvent { state, last }));
        self.in_xr_update = in_xr;
    }

    /// An `XrDepth`: the headset's environment depth. A header alone announces a newly
    /// acquired image: the app's per-pixel tests (occlusion) read that image directly from now
    /// on (`Cx::xr_environment_depth`, its texture imported once per runtime image). Texels
    /// following go to the TSDF depth writer (its own thread integrates them into
    /// `cx.xr_tsdf()`; this never waits).
    pub(crate) fn hal_xr_depth(&mut self, display_ns: u64, bytes: &[u8]) {
        let Some(header) = read_items::<XrDepth>(bytes, 1).next() else { return };
        let texels = (header.width * header.height) as usize;
        let layers = header.layers.max(1) as usize;
        let Some(raw) = bytes.get(XrDepth::SIZE..XrDepth::SIZE + texels * layers * 2) else {
            if header.image == 0 { return; }
            let known = self.hal.xr.as_ref().and_then(|xr| xr.env_images.iter().find(|(t, _)| *t == header.image).map(|(_, tex)| tex.clone()));
            let texture = match known {
                Some(t) => t,
                None => {
                    let texture = crate::texture::Texture::new_with_format(self, crate::texture::TextureFormat::VideoYuvPlane);
                    let id = self.hal.frames.external_texture(texture.texture_id().0, texture.texture_id().generation());
                    self.hal.frames.w.texture_import(id, header.image, makepad_hal::texture_handle::XR_DEPTH);
                    let Some(xr) = self.hal.xr.as_mut() else { return };
                    xr.env_images.push((header.image, texture.clone()));
                    texture
                }
            };
            let Some(xr) = self.hal.xr.as_mut() else { return };
            xr.env_depth = Some((texture, header));
            xr.env_depth_ns = display_ns;
            return;
        };
        // (a readback still in flight when the app stopped wanting the field: dropped)
        if !ROOM_FIELD.load(std::sync::atomic::Ordering::Acquire) { return; }
        let all: Vec<u16> = raw.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        let Some(xr) = self.hal.xr.as_mut() else { return };
        if xr.depth_writer.is_none() {
            match makepad_tsdf::DepthWriter::new() {
                Ok(writer) => xr.depth_writer = Some(writer),
                Err(e) => {
                    crate::error!("XR depth: {e}");
                    return;
                }
            }
        }
        // the self-check (every 5 s while a hand is tracked): the tracked hands' palm and index
        // tip projected into eye 0's image, the image's distance there against the joint's own
        if xr.hand_check_at.map_or(true, |t| t.elapsed().as_secs() >= 5) {
            let checks: Vec<String> = [&xr.last.left_hand, &xr.last.right_hand].into_iter().filter(|h| h.in_view())
                .flat_map(|h| [0usize, 10].map(|j| h.joints[j].position))
                .filter_map(|p| env_depth_at(&header, &all, [p.x, p.y, p.z]).map(|(z, img)| format!("joint {z:.2} m image {}", img.map_or("none".to_string(), |d| format!("{d:.2} m")))))
                .collect();
            if !checks.is_empty() {
                xr.hand_check_at = Some(std::time::Instant::now());
                crate::log!("xr depth hand check (palm, index tip; eye 0, nearest within 12 texels; near {:.2} far {:.2}): {}", header.near, header.far, checks.join(", "));
            }
        }
        // the room field integrates eye 0's image
        let view = |e: &makepad_hal::XrEye| makepad_tsdf::DepthFrameView { orientation: e.pose.orientation, position: e.pose.position, fov: e.fov };
        let mut depth = all;
        depth.truncate(texels);
        let frame = makepad_tsdf::DepthFrame {
            width: header.width,
            height: header.height,
            eye: 0,
            near: header.near,
            far: header.far,
            views: [view(&header.views[0]), view(&header.views[1])],
            depth,
        };
        if let Some(writer) = xr.depth_writer.as_mut() {
            let _ = writer.submit(frame);
        }
    }

    /// `Cx::xr_set_scene_to_room`.
    pub(crate) fn hal_xr_scene_to_room(&mut self, m: Mat4f) {
        if let Some(xr) = self.hal.xr.as_mut() { xr.scene_to_room = m; }
    }

    /// The uniforms of eye `i` for an XR pass: its own uniforms with that eye's camera (left
    /// and right fields alike: each eye is a pass of its own).
    pub(crate) fn hal_xr_eye_uniforms(&mut self, base: &DrawPassUniforms, i: usize) -> Option<&DrawPassUniforms> {
        let xr = self.hal.xr.as_mut()?;
        let frame = xr.frame?;
        let (view, projection) = eye_matrices(&frame, i);
        let view = Mat4f::mul(&view, &xr.scene_to_room);
        let inv = view.invert();
        let msaa = xr.msaa as f32;
        let u = &mut xr.eye_uniforms[i];
        *u = *base;
        u.msaa_samples = msaa;
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
        let view = Mat4f::mul(&view, &xr.scene_to_room);
        let view_r = Mat4f::mul(&view_r, &xr.scene_to_room);
        let msaa = xr.msaa as f32;
        let u = &mut xr.eye_uniforms[0];
        *u = *base;
        u.msaa_samples = msaa;
        u.camera_view = view; u.camera_view_r = view_r;
        u.camera_projection = projection; u.camera_projection_r = projection_r;
        u.depth_view = view; u.depth_view_r = view_r;
        u.depth_projection = projection; u.depth_projection_r = projection_r;
        u.camera_inv = view.invert(); u.camera_inv_r = view_r.invert();
        Some(&xr.eye_uniforms[0])
    }
}
