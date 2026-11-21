use makepad_widgets::makepad_platform::video_file::{VideoColor, VideoMatrix, VideoRange};
use makepad_widgets::*;
use std::collections::VecDeque;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawCameraDisplay = mod.std.set_type_default() do #(DrawCameraDisplay::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        sensor_uv: varying(vec2f)
        vertex: fn() {
            self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
            let uv = self.screen_to_sensor * vec4(self.pos, 0.0, 1.0)
            self.sensor_uv = uv.xy
        }
        pixel: fn() { return self.image.sample(self.sensor_uv) }
    }

    // The cube, hidden where the scene's measured depth (LiDAR) is nearer than the cube.
    mod.draw.DrawArCube = mod.std.set_type_default() do #(DrawArCube::script_shader(vm)){
        ..mod.draw.DrawCube
        lidar: texture_2d(float)
        at: fragment_position()
        pixel: fn() {
            if self.occlude > 0.5 {
                // The composite target is the sensor picture: its pixel is the depth map's uv,
                // and 1 / w is the fragment's distance along the camera's -z (as LiDAR's is).
                let real = self.lidar.sample(self.at.xy / self.target_size).r
                if real > 0.0 && real < 1.0 / self.at.w - #(OCCLUDE_MARGIN_M) {
                    discard()
                }
            }
            return self.lit_color
        }
    }

    mod.widgets.ArView = set_type_default() do #(ArView::register_widget(vm)){
        width: Fill height: Fill
        // The background fills the sensor target even though this pass has the camera's projection.
        background +: {
            image: texture_2d(float)
            depth_write: false
            vertex: fn() {
                self.pos = self.geom.pos
                self.vertex_pos = vec4(self.pos.x * 2.0 - 1.0, 1.0 - self.pos.y * 2.0, 0.99999, 1.0)
            }
            fragment: fn() { self.fb0 = self.image.sample(self.pos) }
        }
        // A frame's depth map (linear metres in .r) copied into the ring beside its picture.
        depth_copy +: {
            image: texture_2d(float)
            pixel: fn() { return vec4(self.image.sample(self.pos).r, 0.0, 0.0, 1.0) }
        }
        cube +: { color: #xfba448 cube_size: vec3(0.1, 0.1, 0.1) }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                show_caption_bar: false
                window.title: "AR"
                body +: {
                    flow: Overlay
                    align: Align{x: 0.5 y: 0.5}
                    mod.widgets.ArView{}
                    unavailable := Label{text: "No world tracking on this device"}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCameraDisplay {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    screen_to_sensor: Mat4f,
}

/// A real surface hides the cube only when it is this much nearer (metres): LiDAR noise on a
/// flat table must not cut holes in a cube standing on it.
const OCCLUDE_MARGIN_M: f32 = 0.015;

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawArCube {
    #[deref]
    cube: DrawCube,
    /// The composite target's size in pixels (the fragment position to depth-map uv).
    #[live]
    target_size: Vec2f,
    /// 1 while the drawn picture has a depth map.
    #[live]
    occlude: f32,
}

/// Normalized portrait window coordinates to native sensor coordinates. The quad and a
/// tap on that quad use the very same matrix, including the crop about the image centre.
fn screen_to_sensor(window: Vec2d, image: [u32; 2]) -> Mat4f {
    let screen_aspect = (window.x / window.y) as f32;
    let rotated_aspect = image[1] as f32 / image[0] as f32;
    let sx = (screen_aspect / rotated_aspect).min(1.0);
    let sy = (rotated_aspect / screen_aspect).min(1.0);
    Mat4f { v: [
        0.0, -sx, 0.0, 0.0,
        sy, 0.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        (1.0 - sy) * 0.5, (1.0 + sx) * 0.5, 0.0, 1.0,
    ] }
}

const RING_SIZE: usize = 8;
// ArFrameEvent::picture is guaranteed to belong to its capture for four sequences.
#[cfg(not(target_arch = "wasm32"))]
const PICTURE_LIFETIME: u64 = 4;

struct Picture {
    frame: ArFrameEvent,
    luma: Texture,
    chroma: Texture,
    /// The frame's depth map (R32F, linear metres), on a device with scene depth.
    depth: Option<Texture>,
}

struct Slot {
    copy: RenderTarget,
    /// The frame's depth map, copied with its picture (the HAL recycles both after four
    /// sequences); `has_depth` says whether this slot's frame had one.
    depth: RenderTarget,
    has_depth: bool,
    frame: Option<ArFrameEvent>,
    /// Repaint in which we recorded the copy. Its draw list owns the imported planes.
    recording: Option<u64>,
}

impl Slot {
    fn record(&mut self, cx: &mut Cx2d, draw: &mut DrawNv12, depth_copy: &mut DrawQuad, picture: Picture) {
        let frame = picture.frame;
        let [width, height] = frame.image_size;
        let size = ((width / 2).max(1), (height / 2).max(1));
        self.copy.ensure_kept(cx, size, RenderTargetFormat::Bgra8);
        self.copy.begin(cx, vec4(0.0, 0.0, 0.0, 1.0));
        let color = VideoColor { matrix: VideoMatrix::Bt601, range: VideoRange::Full, ..VideoColor::BT709 };
        draw.set_planes(&picture.luma, &picture.chroma, width, height, color, false);
        draw.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(size.0 as f64, size.1 as f64) });
        draw.draw_vars.empty_texture(0);
        draw.draw_vars.empty_texture(1);
        self.copy.end(cx);
        self.has_depth = false;
        #[cfg(not(target_arch = "wasm32"))]
        if let (Some(texture), Some(map)) = (&picture.depth, frame.depth) {
            let size = (map.width.max(1), map.height.max(1));
            self.depth.ensure_kept(cx, size, RenderTargetFormat::Rgba16f);
            self.depth.begin(cx, vec4(0.0, 0.0, 0.0, 1.0));
            depth_copy.draw_vars.set_texture(0, texture);
            depth_copy.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(size.0 as f64, size.1 as f64) });
            depth_copy.draw_vars.empty_texture(0);
            self.depth.end(cx);
            self.has_depth = true;
        }
        self.frame = Some(frame);
        self.recording = Some(cx.repaint_id());
    }

    fn release_painted_planes(&mut self, cx: &mut Cx) -> bool {
        let Some(at) = self.recording else { return false };
        if !self.copy.pass.painted_whole_since(cx, at) || (self.has_depth && !self.depth.pass.painted_whole_since(cx, at)) {
            return false;
        }
        // Retire just this copy's recorded texture bindings. The kept targets and their
        // frame metadata stay in the ring; the next copy reuses the lists' storage.
        for target in [&self.copy, &self.depth] {
            if std::ptr::eq(target, &self.depth) && !self.has_depth { continue; }
            target.pass.withdraw(cx);
            let items = &mut cx.draw_lists[target.list.id()].draw_items;
            items.clear();
            items.finish_recording();
        }
        self.recording = None;
        true
    }
}

struct CameraRing {
    slots: [Slot; RING_SIZE],
    next: usize,
    filled: usize,
    delay: usize,
    composite: RenderTarget,
}

impl CameraRing {
    fn new(cx: &mut Cx) -> Self {
        let delay = std::env::var("AR_DELAY").ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
        Self {
            slots: std::array::from_fn(|_| Slot {
                copy: RenderTarget::new(cx, "AR camera copy"),
                depth: RenderTarget::new(cx, "AR depth copy"),
                has_depth: false,
                frame: None,
                recording: None,
            }),
            next: 0,
            filled: 0,
            delay: delay.min(RING_SIZE - 1),
            composite: RenderTarget::new(cx, "AR camera and cube"),
        }
    }

    fn record_pictures(&mut self, cx: &mut Cx2d, draw: &mut DrawNv12, depth_copy: &mut DrawQuad, pending: &mut VecDeque<Picture>) {
        while self.slots[self.next].recording.is_none() {
            let Some(picture) = pending.pop_front() else { break };
            self.slots[self.next].record(cx, draw, depth_copy, picture);
            self.next = (self.next + 1) % RING_SIZE;
            self.filled = (self.filled + 1).min(RING_SIZE);
        }
    }

    fn display_index(&self) -> Option<usize> {
        let oldest_delay = self.filled.checked_sub(1)?;
        Some((self.next + RING_SIZE - 1 - self.delay.min(oldest_delay)) % RING_SIZE)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ArView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    convert: DrawNv12,
    #[live]
    background: DrawQuad,
    #[live]
    depth_copy: DrawQuad,
    #[live]
    cube: DrawArCube,
    #[live]
    display: DrawCameraDisplay,
    #[rust]
    ring: Option<CameraRing>,
    #[rust]
    pending: VecDeque<Picture>,
    #[rust]
    cube_pose: Option<Pose>,
    #[rust]
    last_frame: Option<ArFrameEvent>,
    #[rust]
    delivery_delta_ns: i128,
    #[rust]
    frames: u64,
    #[rust]
    dropped: u64,
    #[rust]
    expired: u64,
    #[rust]
    copies: u64,
    #[rust]
    displayed_seq: Option<u64>,
    #[rust]
    display_updates: u64,
    #[rust]
    report_timer: Timer,
    /// `AR_CAPTURE_EVERY` seconds (default 5, 0 off): see [`ArView::request_capture`].
    #[rust]
    capture_every: u64,
    #[rust]
    capture_tick: u64,
    #[rust]
    capture_read: Option<(ReadbackTicket, u64)>,
    #[rust]
    captures: u64,
    #[rust]
    capture_worker: Option<SyncSender<Capture>>,
    /// `AR_NO_IMPORT=1`: poses only, no picture imported or copied (a memory bisection switch).
    #[rust]
    no_import: bool,
    #[rust]
    reports: u64,
    /// Capture to drawn, of the picture last drawn (the pose's age when its picture is drawn).
    #[rust]
    drawn_age_ns: (i128, i128, u32),
    #[rust]
    had_depth: bool,
    /// `AR_CUBE_M` (default 0.5): how far along the line of sight the cube first comes, and
    /// where a tap that finds no surface puts it.
    #[rust]
    place_distance: f32,
}

/// A read-back composite on its way to a file.
struct Capture {
    path: std::path::PathBuf,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

/// One long-lived worker encodes and writes captures, so the UI thread never encodes a PNG.
fn capture_worker() -> SyncSender<Capture> {
    let (tx, rx) = sync_channel::<Capture>(1);
    let _ = std::thread::Builder::new().name("ar-capture".into()).spawn(move || {
        use makepad_widgets::makepad_zune_png::{
            makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
            PngEncoder,
        };
        for c in rx {
            let options = EncoderOptions::default().set_width(c.width).set_height(c.height)
                .set_depth(BitDepth::Eight).set_colorspace(ColorSpace::RGBA);
            let mut out = Vec::new();
            let written = PngEncoder::new(&c.rgba, options).encode(&mut out)
                .map_err(|e| format!("{e:?}"))
                .and_then(|_| std::fs::write(&c.path, &out).map_err(|e| e.to_string()));
            match written {
                Ok(()) => log!("AR capture written {} ({}x{})", c.path.display(), c.width, c.height),
                Err(e) => log!("AR capture {}: {e}", c.path.display()),
            }
        }
    });
    tx
}

impl ArView {
    #[cfg(not(target_arch = "wasm32"))]
    fn receive(&mut self, cx: &mut Cx, frame: ArFrameEvent) {
        use makepad_widgets::makepad_platform::{makepad_hal::clock_ns, video_file::PicturePlane};
        self.delivery_delta_ns = i128::from(clock_ns()) - i128::from(frame.time_ns);
        self.frames += 1;
        if let Some(last) = self.last_frame {
            self.dropped += frame.seq.saturating_sub(last.seq).saturating_sub(1);
        }
        self.last_frame = Some(frame);
        // A stalled window can still receive captures. Once the HAL can recycle a picture,
        // an unrecorded copy can no longer establish that its pixels match its pose.
        while self.pending.front().is_some_and(|p| frame.seq.saturating_sub(p.frame.seq) >= PICTURE_LIFETIME) {
            self.pending.pop_front();
            self.expired += 1;
        }
        if self.cube_pose.is_none() && frame.tracking == ArTracking::Normal {
            // On the camera's line of sight, however the device is held (an unattended run's
            // captures then show it); a tap moves it onto a surface.
            let forward = frame.camera.orientation.rotate_vec3(&vec3(0.0, 0.0, -1.0));
            // Turned on two axes, so whichever way it is seen two or three faces show, each
            // in its own shade of the cube's fixed light.
            let turn = Quat::multiply(&Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), 0.785), &Quat::from_axis_angle(vec3(1.0, 0.0, 0.0), 0.615));
            self.cube_pose = Some(Pose {
                orientation: turn,
                position: frame.camera.position + forward * self.place_distance,
            });
        }
        if frame.picture != 0 && !frame.image_size.contains(&0) {
            // Import at delivery, while the HAL still holds this sequence. Multiple arrivals
            // before a window draw are copied in arrival order.
            if self.no_import { return; }
            let depth = frame.depth.filter(|d| d.image != 0 && d.width > 0 && d.height > 0);
            if depth.is_some() != self.had_depth {
                self.had_depth = depth.is_some();
                log!("AR depth map {}", depth.map_or("none: the cube is drawn over everything".into(), |d| format!("{}x{}: real things in front hide the cube", d.width, d.height)));
            }
            self.pending.push_back(Picture {
                luma: cx.import_picture_plane(frame.picture, PicturePlane::Luma),
                chroma: cx.import_picture_plane(frame.picture, PicturePlane::Chroma),
                depth: depth.map(|d| cx.import_depth_image(d.image)),
                frame,
            });
            self.area.redraw(cx);
        }
    }

    fn place(&mut self, cx: &mut Cx, point: Vec2d) {
        let rect = self.area.rect(cx);
        if self.ring.as_ref().and_then(CameraRing::display_index).is_none() || rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }
        let x = ((point.x - rect.pos.x) / rect.size.x) as f32;
        let y = ((point.y - rect.pos.y) / rect.size.y) as f32;
        let uv = self.display.screen_to_sensor.transform_vec4(vec4(x, y, 0.0, 1.0));
        if let Some(hit) = cx.ar_raycast(uv.x, uv.y) {
            let up = hit.normal.normalize();
            let axis = vec3(up.z, 0.0, -up.x); // world up crossed with the surface normal
            let sin = axis.length();
            let axis = if sin > 0.0 { axis.normalize() } else { vec3(1.0, 0.0, 0.0) };
            self.cube_pose = Some(Pose {
                orientation: Quat::from_axis_angle(axis, sin.atan2(up.y)),
                position: hit.position + up * (self.cube.cube_size.y * 0.5),
            });
            log!("AR hit uv=({}, {}) position={:?} normal={:?}", uv.x, uv.y, hit.position, hit.normal);
            self.area.redraw(cx);
        } else if let Some(frame) = self.last_frame {
            // No surface there: the cube comes back `place_distance` along the tapped ray (a lost cube is
            // one tap away). The ray through sensor uv from the frame's field of view.
            let [left, right, up, down] = frame.fov.map(f32::tan);
            let ray = vec3(left + uv.x * (right - left), up + uv.y * (down - up), -1.0).normalize();
            let position = frame.camera.position + frame.camera.orientation.rotate_vec3(&ray) * self.place_distance;
            log!("AR tap uv=({}, {}) found no surface: the cube comes to {:?}", uv.x, uv.y, position);
            self.cube_pose = Some(Pose { position, ..self.cube_pose.unwrap_or_default() });
            self.area.redraw(cx);
        }
    }

    /// Every `capture_every` seconds the composite (camera and cube, in sensor orientation) is
    /// read back and written to `<data dir>/ar-captures/frame-<n % 8>.png`, so a device run can
    /// be looked at afterwards (`xcrun devicectl device copy from`, the app data container).
    fn request_capture(&mut self, cx: &mut Cx) {
        self.capture_tick += 1;
        if self.capture_every == 0 || self.capture_tick % self.capture_every != 0 || self.capture_read.is_some() { return; }
        let (Some(ring), Some(frame)) = (&self.ring, self.last_frame) else { return };
        match ring.composite.texture.read_back(cx, ReadbackRequest::default()) {
            Ok(ticket) => self.capture_read = Some((ticket, frame.seq)),
            Err(e) => log!("AR capture not taken: {e:?}"),
        }
    }

    fn take_capture(&mut self, cx: &mut Cx) {
        let Some((ticket, seq)) = self.capture_read else { return };
        let Some(result) = cx.try_take_texture_readbacks_for(&[ticket]).into_iter().next() else { return };
        self.capture_read = None;
        let mut rgba = match result.data {
            Ok(bytes) => bytes.to_vec(),
            Err(e) => return log!("AR capture failed: {e:?}"),
        };
        for px in rgba.chunks_exact_mut(4) { px[3] = 255; }
        let Some(dir) = cx.get_data_dir().map(|d| std::path::Path::new(&d).join("ar-captures")) else { return };
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("frame-{}.png", self.captures % 8));
        self.captures += 1;
        log!("AR capture of seq {seq} -> {} (camera {:?}, cube {:?})", path.display(),
            self.last_frame.map(|f| f.camera.position), self.cube_pose.map(|p| p.position));
        let worker = self.capture_worker.get_or_insert_with(capture_worker);
        match worker.try_send(Capture { path, width: result.width, height: result.height, rgba }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => log!("AR capture skipped: the last one is still being written"),
            Err(TrySendError::Disconnected(_)) => log!("AR capture worker is gone"),
        }
    }

    fn report(&mut self, cx: &Cx) {
        let last = self.last_frame;
        // Where the cube's centre lands: normalized device coordinates of the sensor image
        // (x, y in [-1, 1] and z in [-1, 1] with w > 0 is inside the picture and the depth range).
        if let (Some(f), Some(pose)) = (last, self.cube_pose) {
            let view = f.camera.invert().to_mat4();
            let c = f.projection(0.01, 100.0).transform_vec4(view.transform_vec4(vec4(pose.position.x, pose.position.y, pose.position.z, 1.0)));
            log!("AR cube centre ndc=({:.3}, {:.3}, {:.3}) w={:.3} distance={:.3} m", c.x / c.w, c.y / c.w, c.z / c.w, c.w, (pose.position - f.camera.position).length());
        }
        log!("AR frames={} copies={} display_updates={} dropped_seqs={} expired_before_copy={} tracking={:?} camera={:?} picture={} clock_minus_capture_ns={:?} thermal={:?} footprint_bytes={:?}",
            self.frames, self.copies, self.display_updates, self.dropped, self.expired,
            last.map(|f| f.tracking), last.map(|f| f.camera.position),
            last.map_or(0, |f| f.picture), last.map(|_| self.delivery_delta_ns), cx.thermal_state(), process_footprint_bytes());
        let (sum, max, n) = std::mem::take(&mut self.drawn_age_ns);
        if n > 0 { log!("AR drawn pose age mean {:.1} ms max {:.1} ms over {n} pictures", sum as f64 / n as f64 / 1e6, max as f64 / 1e6); }
        self.reports += 1;
        if self.reports % 30 == 0 {
            let inv = cx.gpu_inventory();
            log!("AR gpu textures={} texture_bytes={} retired_texture_bytes={} draw_lists={} instance_cpu_bytes={} geometries={} device_allocated_bytes={:?}",
                inv.textures.len(), inv.texture_bytes(), inv.retired_texture_bytes, inv.draw_lists.len(),
                inv.instance_cpu_bytes(), inv.geometries, inv.device_allocated_bytes);
        }
        self.frames = 0;
        self.copies = 0;
        self.display_updates = 0;
        self.dropped = 0;
        self.expired = 0;
    }
}

impl Widget for ArView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::Startup if cx.ar_caps().world_tracking => {
                self.ring = Some(CameraRing::new(cx));
                self.no_import = std::env::var_os("AR_NO_IMPORT").is_some();
                log!("AR picture import {}", if self.no_import { "off (AR_NO_IMPORT)" } else { "on" });
                self.capture_every = std::env::var("AR_CAPTURE_EVERY").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
                self.place_distance = std::env::var("AR_CUBE_M").ok().and_then(|s| s.parse().ok()).filter(|d: &f32| *d > 0.0).unwrap_or(0.5);
                self.report_timer = cx.start_interval(1.0);
                cx.ar_start(ArConfig { scene_depth: true, scene_mesh: false });
            }
            #[cfg(not(target_arch = "wasm32"))]
            Event::ArFrame(frame) => self.receive(cx, *frame),
            Event::ArState(event) => {
                log!("AR session {:?}", event.state);
                if !matches!(event.state, ArSessionState::Running(_)) {
                    // The HAL releases its capture buffers when the session leaves Running.
                    self.expired += self.pending.len() as u64;
                    self.pending.clear();
                }
            }
            Event::Signal => self.take_capture(cx),
            Event::Timer(t) if self.report_timer.is_timer(t).is_some() => {
                self.report(cx);
                self.request_capture(cx);
                if let Some(ring) = &self.ring {
                    if ring.slots.iter().any(|s| s.recording.is_some()) {
                        self.area.redraw(cx);
                    }
                }
            }
            Event::Shutdown if self.ring.is_some() => {
                cx.stop_timer(self.report_timer);
                cx.ar_stop();
            }
            _ => {}
        }
        if let Hit::FingerUp(e) = event.hits(cx, self.area) {
            if e.is_over && e.was_tap() {
                self.place(cx, e.abs);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        let Some(ring) = &mut self.ring else { return DrawStep::done() };
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 { return DrawStep::done() }
        for slot in &mut ring.slots {
            if slot.release_painted_planes(cx) { self.copies += 1; }
        }
        ring.record_pictures(cx, &mut self.convert, &mut self.depth_copy, &mut self.pending);
        let Some(index) = ring.display_index() else { return DrawStep::done() };
        let slot = &ring.slots[index];
        let frame = slot.frame.unwrap();
        let size = slot.copy.size();
        let sensor_rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(size.0 as f64, size.1 as f64) };

        let scene = &mut ring.composite;
        scene.ensure(cx, size, RenderTargetFormat::Bgra8);
        let depth = scene.depth(cx);
        scene.pass.set_depth_texture(cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        scene.pass.set_keep_camera_matrix(cx, true);
        scene.pass.set_camera(cx, frame.camera.invert().to_mat4(), frame.projection(0.01, 100.0));
        scene.begin(cx, vec4(0.0, 0.0, 0.0, 1.0));
        for slot in &ring.slots {
            if slot.recording.is_some() {
                cx.make_child_pass(&slot.copy.pass);
                if slot.has_depth { cx.make_child_pass(&slot.depth.pass); }
            }
        }
        self.background.draw_vars.set_texture(0, &slot.copy.texture);
        self.background.draw_abs(cx, sensor_rect);
        if let Some(pose) = self.cube_pose {
            self.cube.transform = pose.to_mat4();
            self.cube.target_size = vec2(size.0 as f32, size.1 as f32);
            self.cube.occlude = if slot.has_depth { 1.0 } else { 0.0 };
            if slot.has_depth { self.cube.draw_vars.set_texture(0, &slot.depth.texture); }
            self.cube.draw(cx);
        }
        scene.end(cx);

        self.display.screen_to_sensor = screen_to_sensor(rect.size, frame.image_size);
        self.display.draw_vars.set_texture(0, &scene.texture);
        self.display.draw_abs(cx, rect);
        if self.displayed_seq.replace(frame.seq) != Some(frame.seq) {
            self.display_updates += 1;
            #[cfg(not(target_arch = "wasm32"))]
            {
                let age = i128::from(makepad_widgets::makepad_platform::makepad_hal::clock_ns()) - i128::from(frame.time_ns);
                let (sum, max, n) = self.drawn_age_ns;
                self.drawn_age_ns = (sum + age, max.max(age), n + 1);
            }
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::Startup) {
            let unsupported = !cx.ar_caps().world_tracking;
            self.ui.widget(cx, ids!(unavailable)).set_visible(cx, unsupported);
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
