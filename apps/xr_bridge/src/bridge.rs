use crate::ipc::{Reply, Request, Worker};
use makepad_widgets::*;
use makepad_widgets::makepad_draw::render_target::{RenderTarget, RenderTargetFormat};
use makepad_widgets::makepad_platform::{foreign_dmabuf::{ForeignDmabuf, ForeignDmabufPlane}, makepad_hal::{XrPose, clock_ns}, video_file::GpuCompletion};
use makepad_xr_runtime::protocol::{self as wire, Message};
use makepad_xr_stream::{XrServer, server::external::{ExternalEye, ExternalFrame, ExternalRenderer, TrackingSnapshot, XrRenderEye}};
use std::{collections::HashMap, os::fd::OwnedFd, sync::Arc, time::Instant};

struct Image { texture: Texture, size: [u32; 2] }
struct Frame { tracking: Arc<TrackingSnapshot>, eyes: [wire::SubmittedEye; 2], alpha: bool }
pub struct Bridge {
    server: XrServer,
    renderer: ExternalRenderer,
    worker: Worker,
    anchor: RenderTarget,
    anchor_list: DrawList2d,
    images: HashMap<u64, Image>,
    tracking: Option<Arc<TrackingSnapshot>>,
    last_token: Option<u64>,
    waiting: bool,
    frame: Option<Frame>,
    requested: bool,
    due: bool,
    drawn: Option<u64>,
    completion: Option<(u64, GpuCompletion)>,
    // A minimum one-way clock-offset estimate. The headset's original times are
    // kept separately for reprojection; this estimate includes minimum transit
    // latency and is not a synchronized clock / latency measurement.
    clock_offset: Option<i128>,
    last_display: i64,
    game: Option<String>,
    streaming: bool,
    pub status: String,
    measured: Instant,
    frames: u64,
    pack_us: u128,
}
fn pose(p: XrPose) -> wire::Pose { wire::Pose { orientation: p.orientation, position: p.position } }
impl Bridge {
    pub fn new(cx: &mut Cx) -> Result<Self, String> {
        let (server, renderer) = XrServer::install_external(cx, "xr-bridge").ok_or("XR streaming is unavailable on this renderer")?;
        let worker = Worker::start(cx).map_err(|e| e.to_string())?;
        let mut anchor = RenderTarget::new(cx, "xr_external_anchor");
        anchor.ensure(cx, (1, 1), RenderTargetFormat::Bgra8);
        Ok(Self { server, renderer, worker, anchor, anchor_list: DrawList2d::new(cx), images: HashMap::new(), tracking: None, last_token: None, waiting: false,
            frame: None, requested: false, due: false, drawn: None, completion: None, clock_offset: None, last_display: 0, game: None, streaming: false,
            status: "Waiting for the headset and a game.".into(), measured: Instant::now(), frames: 0, pack_us: 0 })
    }
    fn answer(&mut self, message: Option<Message>) { self.worker.reply(Reply::message(message)); }
    fn error(&mut self, error: String) {
        eprintln!("xr bridge: {error}");
        self.status = error.clone();
        self.waiting = false;
        self.frame = None;
        self.due = false;
        self.requested = false;
        self.answer(Some(Message::Error(error)));
    }
    fn packet(&mut self, cx: &mut Cx, message: Message, fd: Option<OwnedFd>) -> Result<(), String> {
        match message {
            Message::Hello { version, app } => {
                if version != wire::VERSION { return Err("The game runtime and XR Bridge have different versions.".into()); }
                let headset = self.server.session_eye().zip(self.server.session_refresh_mhz()).zip(cx.foreign_dmabuf_support())
                    .map(|((eye, refresh_mhz), gpu)| wire::Headset { eye, refresh_mhz, render_device: gpu.render_device, formats: gpu.formats });
                if headset.is_some() { self.status = format!("Ready for {app}"); self.game = Some(app); }
                self.answer(Some(Message::Headset(headset)));
            }
            Message::WaitFrame => {
                if self.game.is_none() { return Err("Connect a headset before starting the game.".into()); }
                self.waiting = true;
            }
            Message::Import(image) => {
                if self.images.contains_key(&image.id) { return Err("Duplicate XR image registration".into()); }
                if image.width == 0 || image.height == 0 { return Err("Empty XR image".into()); }
                let texture = cx.foreign_dmabuf_import(ForeignDmabuf { width: image.width, height: image.height,
                    fourcc: image.fourcc, modifier: image.modifier,
                    planes: vec![ForeignDmabufPlane { fd: fd.ok_or("Missing XR image descriptor")?, offset: image.offset, stride: image.stride }] })?;
                self.images.insert(image.id, Image { texture, size: [image.width, image.height] });
                self.answer(Some(Message::Imported { id: image.id }));
            }
            Message::Forget { id } => { self.images.remove(&id); self.answer(None); }
            Message::Frame { token, eyes, alpha } => {
                let tracking = self.tracking.as_ref().filter(|t| t.seq == token).ok_or("XR frame has no matching tracking prediction")?.clone();
                for eye in &eyes {
                    let image = self.images.get(&eye.image).ok_or("XR frame uses an unregistered image")?;
                    cx.foreign_dmabuf_acquire(&image.texture)?;
                }
                self.frame = Some(Frame { tracking, eyes, alpha });
                self.requested = false;
            }
            Message::Empty { token } => {
                if self.tracking.as_ref().is_none_or(|t| t.seq != token) { return Err("Empty XR frame has no matching prediction".into()); }
                self.tracking = None;
                self.answer(None);
            }
            Message::Haptic { hand, amplitude, duration_ns } => {
                if hand > 1 || !amplitude.is_finite() || !(0.0..=1.0).contains(&amplitude) || duration_ns < 0 { return Err("Invalid XR haptic pulse".into()); }
                cx.xr_haptic(hand as usize, amplitude, duration_ns as f32 / 1e9);
                self.answer(None);
            }
            Message::Stop => self.answer(None),
            _ => return Err("Unexpected XR runtime message".into()),
        }
        Ok(())
    }
    pub fn pump(&mut self, cx: &mut Cx) {
        self.worker.flush();
        // Draw events have returned before reaching this point: their render
        // passes were submitted. Never release a game image from inside draw().
        if let Some(token) = self.drawn.take() {
            match cx.frame_completion() {
                Ok(completion) => self.completion = Some((token, completion)),
                Err(e) => self.error(e),
            }
        }
        if let Some((token, completion)) = &self.completion {
            if let Some(result) = completion.poll() {
                let reply = match result {
                    Ok(()) => Reply { message: Some(Message::Released { token: *token }), fence: completion.sync_file() },
                    Err(e) => Reply::message(Some(Message::Error(e))),
                };
                self.completion = None;
                self.tracking = None;
                self.worker.reply(reply);
            }
        }
        while let Ok(request) = self.worker.requests.try_recv() {
            match request {
                Request::Listening => eprintln!("xr bridge: listening on {}", wire::socket_path().display()),
                Request::Packet(message, fd) => if let Err(e) = self.packet(cx, message, fd) { self.error(e); },
                Request::Disconnected => {
                    self.images.clear(); self.tracking = None; self.last_token = None; self.waiting = false; self.game = None; self.streaming = false;
                    self.clock_offset = None; self.last_display = 0;
                    self.status = "Waiting for a game.".into();
                    self.answer(None);
                }
                Request::Failed(e) => { eprintln!("xr bridge: {e}"); self.status = e; }
            }
        }
        if (self.waiting || self.frame.is_some()) && self.worker.peer_closed() {
            self.error("The game disconnected.".into());
        }
        if (self.waiting || self.frame.is_some()) && (!self.server.streaming() || self.server.session_eye().is_none()) {
            self.error("The headset stream stopped.".into());
        }
        if self.waiting {
            if let Some(tracking) = self.server.tracking() {
                if Some(tracking.seq) != self.last_token && self.renderer.ready() {
                    let offset = tracking.received_ns as i128 - tracking.sample_ns as i128;
                    let offset = self.clock_offset.map_or(offset, |previous| previous.min(offset));
                    self.clock_offset = Some(offset);
                    let period = (1_000_000_000_000u64 / self.server.session_refresh_mhz().unwrap_or(90_000).max(1) as u64) as i64;
                    let display_time = ((tracking.display_ns as i128 + offset).clamp(1, i64::MAX as i128) as i64)
                        .max(self.last_display.saturating_add(1)).max((clock_ns().min(i64::MAX as u64) as i64).saturating_add(1));
                    self.last_display = display_time;
                    let frame = &tracking.frame;
                    let value = wire::Tracking { token: tracking.seq, display_time, period, headset_time: tracking.display_ns,
                        head: pose(frame.head), eyes: frame.eyes.map(|e| wire::Eye { pose: pose(e.pose), fov: e.fov }),
                        controllers: frame.controllers.map(|c| wire::Controller { grip_pose: pose(c.grip_pose), aim_pose: pose(c.aim_pose),
                            trigger: c.trigger, grip: c.grip, buttons: c.buttons, stick: c.stick }) };
                    self.last_token = Some(tracking.seq); self.tracking = Some(tracking); self.waiting = false;
                    self.answer(Some(Message::Tracking(value)));
                }
            } else if self.server.session_eye().is_none() {
                self.error("The headset disconnected.".into());
            }
        }
        if let Some(frame) = self.frame.as_ref().filter(|_| !self.requested && self.renderer.ready()) {
            self.requested = self.renderer.request_draw(cx, &frame.tracking);
        }
    }
    pub fn xr_update(&mut self, cx: &mut Cx) {
        if self.frame.is_some() {
            self.due = true;
            cx.redraw_all();
        }
    }
    pub fn draw(&mut self, cx: &mut Cx, event: &DrawEvent) {
        if !self.due { return; }
        let Some(frame) = &self.frame else { return; };
        let eye = |index: usize| {
            let submitted = &frame.eyes[index];
            let image = &self.images[&submitted.image];
            ExternalEye { color: &image.texture, depth: None, size: image.size,
                rendered: XrRenderEye { orientation: submitted.eye.pose.orientation, position: submitted.eye.pose.position, fov: submitted.eye.fov } }
        };
        let started = Instant::now();
        let result = {
            self.anchor.pass.set_size(cx, dvec2(1.0, 1.0));
            self.anchor.pass.set_color_texture(cx, &self.anchor.texture, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&self.anchor.pass, Some(1.0));
            self.anchor_list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(1.0, 1.0), Layout::default());
            let result = self.renderer.present(&mut cx, &ExternalFrame { tracking: &frame.tracking, eyes: [eye(0), eye(1)], alpha: frame.alpha, focus_m: 2.0 });
            cx.end_pass_sized_turtle();
            self.anchor_list.end(&mut cx);
            cx.end_pass(&self.anchor.pass);
            result
        };
        match result {
            Ok(true) => {
                self.pack_us += started.elapsed().as_micros(); self.frames += 1;
                self.drawn = Some(frame.tracking.seq); self.frame = None; self.due = false; self.requested = false;
                if !self.streaming {
                    if let Some(game) = &self.game { self.status = format!("Streaming {game}"); }
                    self.streaming = true;
                }
                SignalToUI::set_ui_signal();
                if self.measured.elapsed().as_secs() >= 5 {
                    eprintln!("xr bridge: {:.1} frames/s, GPU-pack recording {:.3} ms/frame", self.frames as f64 / self.measured.elapsed().as_secs_f64(), self.pack_us as f64 / self.frames.max(1) as f64 / 1000.0);
                    self.frames = 0; self.pack_us = 0; self.measured = Instant::now();
                }
            }
            Ok(false) => (), // Encoder backpressure: retain images and retry on its signal.
            Err(e) => self.error(e),
        }
    }
}
