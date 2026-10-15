//! The camera: the viewfinder over the whole width, the shutter, the
//! front/back switch and the last shot.
//!
//! The viewfinder is the stock `Video` widget with a camera source, in
//! TEXTURE mode on the phone: the camera's frames are textures the Video
//! shader draws, so the viewfinder is part of the app's own picture and goes
//! wherever the window manager puts the app (a native preview is a system
//! view at the activity's coordinates: under the WM it sits in the wrong
//! place and stays put through the WM's animations). [`preview_mode`] is
//! the one switch. The shutter's flash lights the whole viewfinder.
//!
//! A shot is the next camera frame after the shutter (`still`): the app
//! reads the camera's frames on its own slot ([`FRAME_SLOT`]) next to the
//! preview, keeps the one the shutter asked for, and a pool job turns it
//! upright, encodes the JPEG and writes it into the camera roll (`roll`),
//! where Photos picks it up.

use crate::{roll, still};
use makepad_widgets::makepad_platform::permission::{Permission, PermissionStatus};
use makepad_widgets::makepad_platform::thread::{Lane, TaskHandle};
use makepad_widgets::makepad_platform::video::{camera_upright_quarter_turns, CameraBgraFrame, VideoFormat, VideoInputDesc};
use makepad_widgets::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.shader.*

    // The shutter: a white ring around a white disc; the disc gives under
    // the finger (`press`) and greys while a shot is being written (`busy`).
    mod.widgets.DrawShutter = set_type_default() do #(DrawShutter::script_shader(vm)) {
        ..mod.draw.DrawQuad
        press: 0.0
        busy: 0.0
        pixel: fn() {
            let p = self.pos * self.rect_size
            let c = self.rect_size * 0.5
            let r = min(c.x, c.y) - 1.0
            let dist = length(p - c)
            let ring = 1.0 - smoothstep(-0.75, 0.75, abs(dist - (r - 2.5)) - 2.5)
            let inner = (r - 9.0) * (1.0 - 0.12 * self.press)
            let disc = 1.0 - smoothstep(inner - 0.75, inner + 0.75, dist)
            let grey = 1.0 - 0.3 * self.busy - 0.12 * self.press
            let a = max(ring, disc)
            let col = mix(vec3(1.0, 1.0, 1.0), vec3(grey, grey, grey), disc)
            return vec4(col * a, a)
        }
    }

    mod.widgets.CameraViewBase = #(CameraView::register_widget(vm))

    mod.widgets.CameraView = set_type_default() do mod.widgets.CameraViewBase{
        width: Fill
        height: Fill
        flow: Down
        show_bg: true
        draw_bg +: {color: #000}
        draw_shutter: mod.widgets.DrawShutter{}
        draw_flash +: {color: #fff0}
        // The viewfinder, centred at the camera's aspect (`fit_viewfinder`).
        finder := View{
            width: Fill
            height: Fill
            flow: Overlay
            align: Align{x: 0.5 y: 0.5}
            hint := Label{
                text: "Starting the camera…"
                draw_text +: {color: #fffa text_style: theme.font_regular{font_size: 11}}
            }
            viewfinder := Video{
                width: 4
                height: 4
                autoplay: false
            }
        }
        status_row := View{
            width: Fill
            height: 28
            align: Align{x: 0.5 y: 0.5}
            status := Label{
                text: ""
                draw_text +: {color: #fffc text_style: theme.font_regular{font_size: 9}}
            }
        }
        controls := View{
            width: Fill
            height: 124
            flow: Right
            align: Align{x: 0.5 y: 0.5}
            padding: Inset{left: 32 right: 32 bottom: 16}
            // The last shot: opens it in Photos.
            thumb := RoundedView{
                width: 56
                height: 56
                cursor: MouseCursor.Hand
                draw_bg +: {color: #fff2 border_radius: 10.0}
                thumb_image := Image{
                    width: Fill
                    height: Fill
                    fit: ImageFit.CropToFill
                }
            }
            Filler{}
            // Drawn by `draw_shutter` over this rect.
            shutter := View{
                width: 76
                height: 76
                cursor: MouseCursor.Hand
            }
            Filler{}
            switch := RoundedView{
                width: 56
                height: 56
                align: Align{x: 0.5 y: 0.5}
                cursor: MouseCursor.Hand
                draw_bg +: {color: #fff2 border_radius: 28.0}
                Icon{
                    icon_walk: Walk{width: 28 height: 28}
                    draw_icon +: {
                        svg: crate_resource("self:resources/icons/switch.svg")
                        color: #fff
                    }
                }
            }
        }
    }
}

/// The camera slot the app reads frames on. Slot 0 belongs to a native
/// preview on the platforms that have one (preparing it clears slot 0's
/// callbacks), so shots are read on the next.
pub const FRAME_SLOT: usize = 1;

/// How long the flash lasts, seconds.
const FLASH_SECONDS: f64 = 0.35;

/// How long the shutter waits for a frame before saying there is none.
const FRAME_TIMEOUT_SECONDS: f64 = 2.0;

/// The preview the viewfinder runs: the camera's frames as textures on the
/// phone (see the module doc; `Native` puts the system's preview view at
/// the viewfinder's rect instead), the platform's own choice elsewhere
/// (Linux draws textures, macOS shows a native preview).
pub fn preview_mode() -> VideoCameraPreviewMode {
    if cfg!(target_os = "android") {
        VideoCameraPreviewMode::Texture
    } else {
        VideoCameraPreviewMode::Auto
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawShutter {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    press: f32,
    #[live]
    busy: f32,
}

/// A written shot, back from the pool.
pub struct Saved {
    pub path: PathBuf,
    pub thumb: CameraBgraFrame,
}

/// The camera the viewfinder runs: which one, in which format.
#[derive(Clone, Copy)]
struct Running {
    camera: usize,
    format: VideoFormat,
}

impl PartialEq for Running {
    fn eq(&self, other: &Self) -> bool {
        self.camera == other.camera && self.format.format_id == other.format.format_id
    }
}

#[derive(Script, Widget)]
pub struct CameraView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live]
    draw_shutter: DrawShutter,
    #[live]
    draw_flash: DrawColor,
    #[rust]
    started: bool,
    #[rust]
    permission: Option<PermissionStatus>,
    #[rust]
    descs: Vec<VideoInputDesc>,
    /// Indices into `descs` in switch order (back cameras first).
    #[rust]
    order: Vec<usize>,
    /// Position in `order` the person wants.
    #[rust]
    wanted: usize,
    /// What the viewfinder was started with.
    #[rust]
    running: Option<Running>,
    /// The camera's error, shown until the next start.
    #[rust]
    error: Option<String>,
    /// The frame callback keeps the next frame while this is set.
    #[rust]
    armed: Arc<AtomicBool>,
    #[rust]
    held: Arc<Mutex<Option<still::HeldFrame>>>,
    /// When the shutter asked for a frame (the frame has not come yet).
    #[rust]
    asked_at: Option<f64>,
    /// The camera's quarter turns when the shutter was pressed.
    #[rust]
    asked_turns: usize,
    #[rust]
    saving: Vec<TaskHandle<Result<Saved, String>>>,
    #[rust]
    last_shot: Option<PathBuf>,
    #[rust]
    thumb_texture: Option<Texture>,
    #[rust]
    flash_at: Option<f64>,
    #[rust]
    press: f32,
    #[rust]
    next: NextFrame,
    /// The viewfinder size last applied (w, h).
    #[rust]
    fitted: (f64, f64),
    #[rust]
    status_text: String,
    /// The size of the preview's pictures, as the platform prepared them
    /// (upright: a phone turns them with the display).
    #[rust]
    preview_size: Option<(f64, f64)>,
}

impl ScriptHook for CameraView {}

impl CameraView {
    /// The first event: ask for the camera, start reading frames, show the
    /// newest shot already in the roll.
    fn start(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        cx.request_permission(Permission::Camera);
        let armed = self.armed.clone();
        let held = self.held.clone();
        cx.camera_frame_input(FRAME_SLOT, move |frame| {
            if armed.swap(false, Ordering::AcqRel) {
                *held.lock().unwrap_or_else(|e| e.into_inner()) = Some(still::HeldFrame::keep(frame));
            }
        });
        if let Some(path) = roll::latest_shot(&roll::roll_dir()) {
            let image = self.view.image(cx, ids!(thumb_image));
            if image.load_image_file_by_path_async(cx, &path).is_ok() {
                self.last_shot = Some(path);
            }
        }
    }

    fn set_status(&mut self, cx: &mut Cx, text: String) {
        if self.status_text != text {
            self.view.label(cx, ids!(status)).set_text(cx, &text);
            self.status_text = text;
        }
    }

    fn set_hint(&mut self, cx: &mut Cx, text: &str) {
        self.view.label(cx, ids!(hint)).set_text(cx, text);
    }

    /// The camera and format the person wants, when there is one.
    fn target(&self) -> Option<Running> {
        let camera = *self.order.get(self.wanted % self.order.len().max(1))?;
        let format = still::pick_format(&self.descs[camera], !cfg!(target_os = "android"))?;
        Some(Running { camera, format })
    }

    /// Clockwise quarter turns that make the running camera's frames upright.
    fn quarter_turns(&self) -> usize {
        match self.running {
            Some(run) => {
                let desc = &self.descs[run.camera];
                camera_upright_quarter_turns(desc.sensor_orientation, still::is_front(desc))
            }
            None => 0,
        }
    }

    /// Bring the viewfinder to what the person wants: stop a camera that
    /// is not wanted any more, start the wanted one once the old is gone.
    fn drive(&mut self, cx: &mut Cx) {
        if self.permission != Some(PermissionStatus::Granted) {
            let text = match self.permission {
                Some(PermissionStatus::DeniedPermanent) | Some(PermissionStatus::DeniedCanRetry) => "Camera access is off for makepad",
                _ => "Waiting for camera access…",
            };
            self.set_hint(cx, text);
            return;
        }
        let video = self.view.video(cx, ids!(viewfinder));
        let target = self.target();
        if self.running.is_some() && self.running == target && !video.is_unprepared() {
            return;
        }
        if !video.is_unprepared() {
            if !video.is_cleaning_up() {
                log!("camera: stopping the viewfinder for a camera switch");
                // The shot reader lets go of the old camera too: a phone
                // opens one camera at a time.
                cx.use_video_input(&[]);
                video.stop_and_cleanup_resources(cx);
            }
            return;
        }
        self.running = None;
        let Some(target) = target else {
            self.set_hint(cx, if self.descs.is_empty() { "No camera found" } else { "This camera has no picture format the app can read" });
            return;
        };
        let desc = &self.descs[target.camera];
        let (input, format) = (desc.input_id, target.format.format_id);
        video.set_camera_preview_mode(cx, preview_mode());
        video.set_source_camera(cx, input, format);
        video.begin_playback(cx);
        // The shot reader runs the same camera and format as the preview,
        // so both share one capture.
        cx.use_video_input(&vec![(input, format); FRAME_SLOT + 1]);
        self.running = Some(target);
        self.preview_size = None;
        self.error = None;
        self.fitted = (0.0, 0.0);
        let name = desc.name.clone();
        log!("camera: starting {name} at {}x{}", target.format.width, target.format.height);
        self.set_status(cx, format!("{name} · {}×{}", target.format.width, target.format.height));
        self.set_hint(cx, "Starting the camera…");
        self.view.redraw(cx);
    }

    /// Size the viewfinder to the camera's upright aspect inside the frame.
    fn fit_viewfinder(&mut self, cx: &mut Cx) {
        let Some(run) = self.running else { return };
        let frame = self.view.widget(cx, ids!(finder)).area().rect(cx);
        if frame.size.x < 1.0 || frame.size.y < 1.0 {
            return;
        }
        let (fw, fh) = (run.format.width as f64, run.format.height as f64);
        let aspect = match self.preview_size {
            Some((w, h)) if w >= 1.0 && h >= 1.0 => w / h,
            _ if self.quarter_turns() % 2 == 1 => fh / fw,
            _ => fw / fh,
        };
        let (w, h) = if frame.size.x / frame.size.y > aspect {
            (frame.size.y * aspect, frame.size.y)
        } else {
            (frame.size.x, frame.size.x / aspect)
        };
        let (w, h) = (w.floor(), h.floor());
        if (w, h) == self.fitted {
            return;
        }
        self.fitted = (w, h);
        let mut video = self.view.widget(cx, ids!(viewfinder));
        script_apply_eval!(cx, video, { width: #(w) height: #(h) });
        self.view.redraw(cx);
    }

    /// The shutter: keep the next frame.
    fn shoot(&mut self, cx: &mut Cx) {
        if self.running.is_none() || self.asked_at.is_some() {
            return;
        }
        self.asked_turns = self.quarter_turns();
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.armed.store(true, Ordering::Release);
        let now = Cx::monotonic_now();
        self.asked_at = Some(now);
        self.flash_at = Some(now);
        cx.haptic_feedback(HapticFeedback::Click);
        self.next = cx.new_next_frame();
        self.view.redraw(cx);
    }

    /// A frame the shutter asked for arrived: develop, encode and write it
    /// off the UI thread.
    fn poll_frame(&mut self, cx: &mut Cx) {
        let Some(asked) = self.asked_at else { return };
        let frame = self.held.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(frame) = frame else {
            if Cx::monotonic_now() - asked > FRAME_TIMEOUT_SECONDS {
                self.armed.store(false, Ordering::Release);
                self.asked_at = None;
                self.set_status(cx, "No picture came from the camera".into());
            }
            return;
        };
        self.asked_at = None;
        let turns = self.asked_turns;
        let time = makepad_civil_time::local_now();
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_millis())
            .unwrap_or(0);
        let job = cx.task_pool().submit(Lane::Heavy, move || {
            let picture = still::develop(&frame, turns)?;
            let jpeg = still::encode_jpeg(&picture, still::JPEG_QUALITY)?;
            let path = roll::save_shot(&roll::roll_dir(), &time, millis, &jpeg)?;
            Ok(Saved { path, thumb: still::thumbnail(&picture, 160) })
        });
        match job {
            Ok(job) => self.saving.push(job),
            Err(e) => self.set_status(cx, format!("Could not save the shot: {e}")),
        }
        self.view.redraw(cx);
    }

    /// Written shots: the thumbnail and the status line.
    fn poll_saving(&mut self, cx: &mut Cx) {
        if self.saving.is_empty() {
            return;
        }
        let mut done = Vec::new();
        self.saving.retain_mut(|job| match job.try_take() {
            Some(result) => {
                done.push(result.map_err(|e| e.to_string()).and_then(|r| r));
                false
            }
            None => true,
        });
        for result in done {
            match result {
                Ok(saved) => {
                    let name = saved.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    self.set_status(cx, format!("Saved {name}"));
                    self.show_thumb(cx, &saved.thumb);
                    self.last_shot = Some(saved.path);
                }
                Err(e) => self.set_status(cx, format!("Could not save the shot: {e}")),
            }
        }
        self.view.redraw(cx);
    }

    fn show_thumb(&mut self, cx: &mut Cx, thumb: &CameraBgraFrame) {
        if thumb.width == 0 || thumb.height == 0 {
            return;
        }
        let texture = self.thumb_texture.get_or_insert_with(|| {
            Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 { width: 1, height: 1, data: Some(vec![0]), updated: TextureUpdated::Full })
        });
        texture.set_data_u32(cx, thumb.width, thumb.height, thumb.pixels.clone());
        let texture = texture.clone();
        self.view.image(cx, ids!(thumb_image)).set_texture(cx, Some(texture));
    }

    /// The last shot, in Photos: the window manager opens (or brings up)
    /// Photos on it; a standalone camera starts the Photos app beside it.
    fn open_last_shot(&mut self, cx: &mut Cx) {
        let Some(path) = self.last_shot.clone() else { return };
        let req = makepad_wm_api::WmRequest::Open { app: Some("photos".into()), path: path.to_string_lossy().to_string() };
        if makepad_wm_api::send(cx, &req) {
            return;
        }
        #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
        if let Some(photos) = makepad_wm_api::sibling_app("photos") {
            if std::process::Command::new(photos).arg(&path).spawn().is_ok() {
                return;
            }
        }
        self.set_status(cx, format!("Photos is not here to open {}", path.display()));
    }

    /// The next camera in switch order.
    fn switch_camera(&mut self, cx: &mut Cx) {
        if self.order.len() < 2 {
            return;
        }
        self.wanted = (self.wanted + 1) % self.order.len();
        self.drive(cx);
    }
}

impl Widget for CameraView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.start(cx);
        // The viewfinder first: it takes the platform's answers about its
        // playback (prepared, released) before `drive` reads its state.
        self.view.handle_event(cx, event, scope);
        match event {
            Event::PermissionResult(result) if result.permission == Permission::Camera => {
                self.permission = Some(result.status);
                self.drive(cx);
            }
            Event::VideoInputs(inputs) => {
                self.descs = inputs.descs.clone();
                self.order = still::camera_order(&self.descs);
                // A camera that went away: the list changed under the run.
                if self.running.is_some_and(|run| run.camera >= self.descs.len()) {
                    self.running = None;
                }
                self.drive(cx);
            }
            Event::VideoPlaybackPrepared(e) => {
                self.preview_size = Some((e.video_width as f64, e.video_height as f64));
                self.view.redraw(cx);
                self.set_hint(cx, "");
                self.drive(cx);
            }
            Event::VideoPlaybackResourcesReleased(_) => {
                log!("camera: the viewfinder let go of its camera");
                self.running = None;
                self.drive(cx);
            }
            Event::VideoDecodingError(e) => {
                self.error = Some(e.error.clone());
                self.set_hint(cx, &format!("Camera error: {}", e.error));
            }
            _ => {}
        }
        if self.next.is_event(event).is_some() {
            let now = Cx::monotonic_now();
            if self.flash_at.is_some_and(|at| now - at > FLASH_SECONDS) {
                self.flash_at = None;
            }
            if self.flash_at.is_some() || self.asked_at.is_some() {
                self.next = cx.new_next_frame();
            }
            self.view.redraw(cx);
        }
        self.poll_frame(cx);
        self.poll_saving(cx);
        if let Event::Actions(actions) = event {
            let shutter = self.view.view(cx, ids!(shutter));
            if shutter.finger_down(actions).is_some() {
                self.press = 1.0;
                self.view.redraw(cx);
            }
            if let Some(up) = shutter.finger_up(actions) {
                self.press = 0.0;
                self.view.redraw(cx);
                if up.is_over && !up.cancelled {
                    self.shoot(cx);
                }
            }
            if self.view.view(cx, ids!(switch)).finger_up(actions).is_some_and(|fe| fe.is_over && !fe.cancelled) {
                self.switch_camera(cx);
            }
            if self.view.view(cx, ids!(thumb)).finger_up(actions).is_some_and(|fe| fe.is_over && !fe.cancelled) {
                self.open_last_shot(cx);
            }
        }
        // Space or Enter takes a shot on a desk.
        if let Event::KeyDown(ke) = event {
            if matches!(ke.key_code, KeyCode::Space | KeyCode::ReturnKey) {
                self.shoot(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.start(cx);
        while self.view.draw_walk(cx, scope, walk).step().is_some() {}
        self.fit_viewfinder(cx);
        // The flash: the frame around the picture lights up and fades.
        if let Some(at) = self.flash_at {
            let t = ((Cx::monotonic_now() - at) / FLASH_SECONDS).clamp(0.0, 1.0) as f32;
            let alpha = (1.0 - t) * (1.0 - t) * 0.9;
            let finder = self.view.widget(cx, ids!(finder)).area().rect(cx);
            self.draw_flash.color = vec4(1.0, 1.0, 1.0, alpha);
            self.draw_flash.draw_abs(cx, finder);
        }
        let shutter = self.view.widget(cx, ids!(shutter)).area().rect(cx);
        self.draw_shutter.press = self.press;
        self.draw_shutter.busy = if self.asked_at.is_some() || !self.saving.is_empty() { 1.0 } else { 0.0 };
        self.draw_shutter.draw_abs(cx, shutter);
        DrawStep::done()
    }
}
