//! Live/realtime session: a permanent streaming job that owns the worker
//! (one GPU = one job) until stopped. `RealtimeSession` is the state shared
//! between the worker thread (running [`run_live`]) and the HTTP route
//! thread (`server::route_loop`, which feeds it control updates, reference
//! images and input frames from any number of connected websockets, and
//! registers/removes their output senders). See `protocol.rs`'s "Realtime
//! session wire protocol" doc block for the wire contract and
//! `realtime_wire.rs` for the (de)serialization helpers this module calls.

use crate::backend::{
    CameraMotion, CancelToken, ContentBackend, LiveConfig, LiveFrameIn, LiveParams, LoopMode,
    OutputEncoding, RgbImage, SeedMode,
};
use crate::error::AssetAiError;
use crate::realtime_wire::{self, ClientMessage, FrameHeader, FrameKind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// One connected realtime websocket: the sender the connection's write
/// thread drains (`platform/network/src/http_server.rs`). The channel is an
/// unbounded `std::sync::mpsc`, so pushing to it never blocks; a closed
/// receiver (client gone) makes `send` return `Err`, which is how a dead
/// socket is detected and dropped — never retried, never blocks the loop.
struct SessionSocket {
    id: u64,
    sender: mpsc::Sender<Vec<u8>>,
}

/// The session-level knobs a control message can also touch, alongside the
/// backend-facing [`LiveConfig`] (see `RealtimeSession::apply_control`).
struct SessionState {
    config: LiveConfig,
    loop_mode: LoopMode,
    output_encoding: OutputEncoding,
    max_fps: f64,
    idle_timeout_s: u64,
}

pub struct RealtimeSession {
    pub job_id: String,
    pub model_id: String,
    state: Mutex<SessionState>,
    mailbox: Mutex<Option<RgbImage>>,
    mailbox_cv: Condvar,
    sockets: Mutex<Vec<SessionSocket>>,
    frames_in: AtomicU64,
    frames_out: AtomicU64,
    dropped: AtomicU64,
    stop_requested: AtomicBool,
}

impl RealtimeSession {
    pub fn new(job_id: String, params: &LiveParams) -> Self {
        Self {
            job_id,
            model_id: params.model.clone(),
            state: Mutex::new(SessionState {
                config: params.config.clone(),
                loop_mode: params.loop_mode,
                output_encoding: params.output_encoding,
                max_fps: params.max_fps,
                idle_timeout_s: params.idle_timeout_s,
            }),
            mailbox: Mutex::new(None),
            mailbox_cv: Condvar::new(),
            sockets: Mutex::new(Vec::new()),
            frames_in: AtomicU64::new(0),
            frames_out: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            stop_requested: AtomicBool::new(false),
        }
    }

    pub fn add_socket(&self, id: u64, sender: mpsc::Sender<Vec<u8>>) {
        self.sockets.lock().unwrap().push(SessionSocket { id, sender });
    }

    pub fn remove_socket(&self, id: u64) {
        self.sockets.lock().unwrap().retain(|socket| socket.id != id);
    }

    pub fn socket_count(&self) -> usize {
        self.sockets.lock().unwrap().len()
    }

    /// Pushes bytes to every connected socket; a dead channel is dropped
    /// instead of retried (see [`SessionSocket`]'s doc). Never blocks.
    pub fn push_bytes(&self, bytes: Vec<u8>) {
        let mut sockets = self.sockets.lock().unwrap();
        sockets.retain(|socket| socket.sender.send(bytes.clone()).is_ok());
    }

    /// Sends the empty-payload close sentinel to every socket (see
    /// `http_server::handle_web_socket`'s write thread, which shuts the TCP
    /// connection down on an empty push) and forgets them.
    pub fn close_all_sockets(&self) {
        let mut sockets = self.sockets.lock().unwrap();
        for socket in sockets.drain(..) {
            let _ = socket.sender.send(Vec::new());
        }
    }

    /// `{"type":"stop"}` — also wakes a `loop_mode = "feed"` iteration
    /// blocked waiting for an input frame that will now never come.
    pub fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Relaxed);
        self.mailbox_cv.notify_all();
    }

    pub fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::Relaxed)
    }

    /// Replaces the mailbox's unconsumed frame (if any): the session only
    /// ever keeps the LATEST pushed input frame, incrementing `dropped` when
    /// it overwrites one nothing consumed yet.
    pub fn push_input_frame(&self, image: RgbImage) {
        let mut mailbox = self.mailbox.lock().unwrap();
        if mailbox.is_some() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        *mailbox = Some(image);
        self.frames_in.fetch_add(1, Ordering::Relaxed);
        self.mailbox_cv.notify_all();
    }

    fn take_mailbox_frame(&self) -> Option<RgbImage> {
        self.mailbox.lock().unwrap().take()
    }

    fn wait_for_mailbox(&self, timeout: Duration) {
        let guard = self.mailbox.lock().unwrap();
        let _ = self.mailbox_cv.wait_timeout(guard, timeout);
    }

    fn idle_timeout_s(&self) -> u64 {
        self.state.lock().unwrap().idle_timeout_s
    }

    /// A cloned snapshot of everything `run_live` needs for one iteration
    /// (cloning `LiveConfig` — including any reference images — up front so
    /// the model call below never holds the session lock).
    fn snapshot(&self) -> (LiveConfig, LoopMode, OutputEncoding, f64) {
        let state = self.state.lock().unwrap();
        (
            state.config.clone(),
            state.loop_mode,
            state.output_encoding,
            state.max_fps,
        )
    }

    /// Merges a partial `{"type":"control", ...}` update: only the fields
    /// present in `update` change anything (see [`apply_control_to_config`]
    /// for the `LiveConfig` subset; the session-only knobs are merged here).
    pub fn apply_control(&self, update: &realtime_wire::ControlUpdateJson) {
        let mut state = self.state.lock().unwrap();
        apply_control_to_config(&mut state.config, update);
        if let Some(mode) = update
            .loop_mode
            .as_deref()
            .and_then(|text| LoopMode::parse(text).ok())
        {
            state.loop_mode = mode;
        }
        if let Some(encoding) = update
            .output_encoding
            .as_deref()
            .and_then(|text| OutputEncoding::parse(text).ok())
        {
            state.output_encoding = encoding;
        }
        if let Some(fps) = update.max_fps {
            if fps.is_finite() {
                state.max_fps = fps.max(0.0).min(240.0);
            }
        }
        if let Some(timeout) = update.idle_timeout_s {
            state.idle_timeout_s = timeout.min(3600);
        }
    }

    /// `{"type":"reference", "slot":N, ...}`: grows `references` with black
    /// placeholders up to `slot` if needed, then sets it.
    pub fn set_reference(&self, slot: usize, image: RgbImage) {
        let mut state = self.state.lock().unwrap();
        if state.config.references.len() <= slot {
            state
                .config
                .references
                .resize_with(slot + 1, || RgbImage::blank(1, 1));
        }
        state.config.references[slot] = image;
    }

    /// Handles one client -> server binary message: decodes it as an input
    /// frame and pushes it to the mailbox.
    pub fn handle_binary(&self, bytes: &[u8]) -> Result<(), AssetAiError> {
        let (header, payload) = realtime_wire::decode_frame(bytes)?;
        let image = decode_frame_payload(header, payload)?;
        self.push_input_frame(image);
        Ok(())
    }

    /// Handles one client -> server text message: control / reference / stop.
    pub fn handle_text(&self, text: &str) -> Result<(), AssetAiError> {
        match realtime_wire::parse_client_message(text)? {
            ClientMessage::Control(update) => self.apply_control(&update),
            ClientMessage::Reference(reference) => {
                let slot = reference.slot.unwrap_or(0) as usize;
                let png_b64 = reference.png_b64.as_deref().unwrap_or("");
                let bytes = makepad_base64::base64_decode(png_b64.as_bytes())
                    .map_err(|e| AssetAiError::Params(format!("reference: bad base64: {e:?}")))?;
                let (data, width, height) = crate::testpattern::decode_png_rgb8(&bytes)?;
                self.set_reference(slot, RgbImage { width, height, data });
            }
            ClientMessage::Stop => self.request_stop(),
        }
        Ok(())
    }
}

/// Merges a partial control update into a [`LiveConfig`]: only fields set in
/// `update` change; everything else keeps its current value. Pure and
/// directly unit-testable (no session/lock plumbing) — see the tests below.
pub fn apply_control_to_config(config: &mut LiveConfig, update: &realtime_wire::ControlUpdateJson) {
    if let Some(prompt) = update.prompt.as_ref() {
        config.prompt = prompt.clone();
    }
    if let Some(negative) = update.negative_prompt.as_ref() {
        config.negative_prompt = negative.clone();
    }
    if let Some(strength) = update.strength {
        if strength.is_finite() {
            config.strength = (strength as f32).clamp(0.0, 1.0);
        }
    }
    if let Some(steps) = update.steps {
        config.steps = steps.clamp(1, 200);
    }
    if let Some(guidance) = update.guidance {
        if guidance.is_finite() {
            config.guidance = Some(guidance as f32);
        }
    }
    if let Some(seed) = update.seed {
        config.seed = seed;
    }
    if let Some(mode) = update
        .seed_mode
        .as_deref()
        .and_then(|text| SeedMode::parse(text).ok())
    {
        config.seed_mode = mode;
    }
    if let Some(width) = update.width {
        config.width = width.clamp(16, 4096);
    }
    if let Some(height) = update.height {
        config.height = height.clamp(16, 4096);
    }
    if let Some(camera) = update.camera.as_ref() {
        if let Some(dolly) = camera.dolly {
            config.camera.dolly = dolly as f32;
        }
        if let Some(pan_x) = camera.pan_x {
            config.camera.pan_x = pan_x as f32;
        }
        if let Some(pan_y) = camera.pan_y {
            config.camera.pan_y = pan_y as f32;
        }
        if let Some(roll) = camera.roll {
            config.camera.roll = roll as f32;
        }
    }
}

fn decode_frame_payload(header: FrameHeader, payload: &[u8]) -> Result<RgbImage, AssetAiError> {
    match header.kind {
        FrameKind::Raw => Ok(RgbImage {
            width: header.width as u32,
            height: header.height as u32,
            data: payload.to_vec(),
        }),
        FrameKind::Png => {
            let (data, width, height) = crate::testpattern::decode_png_rgb8(payload)?;
            Ok(RgbImage { width, height, data })
        }
    }
}

fn encode_output_frame(image: &RgbImage, encoding: OutputEncoding, frame_index: u32) -> Vec<u8> {
    let (kind, payload) = match encoding {
        OutputEncoding::Raw => (FrameKind::Raw, image.data.clone()),
        OutputEncoding::Png => {
            match crate::testpattern::encode_png_rgb8(&image.data, image.width as usize, image.height as usize) {
                Ok(bytes) => (FrameKind::Png, bytes),
                Err(e) => {
                    eprintln!("realtime: png output encode failed, falling back to raw: {e}");
                    (FrameKind::Raw, image.data.clone())
                }
            }
        }
    };
    let header = FrameHeader {
        kind,
        width: image.width.min(u16::MAX as u32) as u16,
        height: image.height.min(u16::MAX as u32) as u16,
        frame_index,
    };
    realtime_wire::encode_frame(header, &payload)
}

/// Resolves the per-frame seed from `config.seed_mode`: `Fixed` never
/// changes it; `Increment` adds the frame counter; `Random` is a
/// deterministic (no RNG dependency) hash of the base seed and frame index —
/// reproducible given the same base seed, but not a fixed sequence.
fn resolve_seed(config: &mut LiveConfig, frame_index: u64) {
    config.seed = match config.seed_mode {
        SeedMode::Fixed => config.seed,
        SeedMode::Increment => config.seed.wrapping_add(frame_index),
        SeedMode::Random => splitmix64(config.seed ^ frame_index.wrapping_mul(0x9E3779B97F4A7C15)),
    };
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The feedback-loop camera warp: a plain CPU center-zoom + pan + roll
/// resample (bilinear, clamp-to-edge), applied to the session's own
/// previous output before it becomes the next `live_step` init image in
/// `loop_mode = "feedback"`. This is the ONLY place camera motion is
/// applied — backends never warp `LiveFrameIn::init` themselves (see
/// [`CameraMotion`]'s doc). The CUDA depth-parallax version of this warp is
/// a documented follow-up; this bilinear resample is the CPU stand-in and
/// what `testpattern` relies on for its "zoom" behavior.
pub fn warp_feedback(image: &RgbImage, camera: &CameraMotion) -> RgbImage {
    let width = image.width;
    let height = image.height;
    if width == 0 || height == 0 || image.data.len() != width as usize * height as usize * 3 {
        return image.clone();
    }
    let zoom = (1.0 + camera.dolly * 0.05).max(1.0e-3);
    let (sin_r, cos_r) = camera.roll.sin_cos();
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let mut out = vec![0u8; image.data.len()];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let rx = dx * cos_r + dy * sin_r;
            let ry = -dx * sin_r + dy * cos_r;
            let sx = cx + rx / zoom - camera.pan_x * width as f32;
            let sy = cy + ry / zoom - camera.pan_y * height as f32;
            let rgb = sample_bilinear_clamped(image, sx, sy);
            let idx = (y as usize * width as usize + x as usize) * 3;
            out[idx..idx + 3].copy_from_slice(&rgb);
        }
    }
    RgbImage { width, height, data: out }
}

fn sample_bilinear_clamped(image: &RgbImage, x: f32, y: f32) -> [u8; 3] {
    let max_x = (image.width - 1) as f32;
    let max_y = (image.height - 1) as f32;
    let x = x.clamp(0.0, max_x.max(0.0));
    let y = y.clamp(0.0, max_y.max(0.0));
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width - 1);
    let y1 = (y0 + 1).min(image.height - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let pixel = |px: u32, py: u32| -> [f32; 3] {
        let idx = (py as usize * image.width as usize + px as usize) * 3;
        [
            image.data[idx] as f32,
            image.data[idx + 1] as f32,
            image.data[idx + 2] as f32,
        ]
    };
    let p00 = pixel(x0, y0);
    let p10 = pixel(x1, y0);
    let p01 = pixel(x0, y1);
    let p11 = pixel(x1, y1);
    let mut out = [0u8; 3];
    for c in 0..3 {
        let top = p00[c] * (1.0 - fx) + p10[c] * fx;
        let bottom = p01[c] * (1.0 - fx) + p11[c] * fx;
        let value = top * (1.0 - fy) + bottom * fy;
        out[c] = value.round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// The live-session loop. Runs on the single worker thread in place of
/// `generate` for a live job (`server::execute_live_job`). Loops until:
/// - the job's cancel flag is raised (`POST /job/<id>/cancel`) -> returns
///   `Err(AssetAiError::Cancelled)`, the job ends `cancelled`;
/// - a `{"type":"stop"}` message arrives, or every socket has been gone for
///   `idle_timeout_s` (0 = never) -> returns `Ok(())`, the job ends `done`.
///
/// `progress` is called at up to ~10 Hz (never on every frame) with
/// `(stage, frames_in, frames_out, fps)` so `server::execute_live_job` can
/// mirror it into `JobState::Live` without lock churn; the `stats` JSON
/// message is pushed to every socket on every frame regardless (cheap: it
/// rides the same channel as the frame it describes).
pub fn run_live(
    session: &RealtimeSession,
    backend: &mut dyn ContentBackend,
    cancel: &CancelToken,
    mut progress: impl FnMut(&str, u64, u64, f64),
) -> Result<(), AssetAiError> {
    let mut last_output: Option<RgbImage> = None;
    let mut frame_index: u64 = 0;
    let mut last_progress_push = Instant::now();
    let mut idle_since: Option<Instant> = None;

    loop {
        cancel.check()?;
        if session.stop_requested() {
            return Ok(());
        }

        let idle_timeout_s = session.idle_timeout_s();
        if session.socket_count() == 0 {
            let since = *idle_since.get_or_insert_with(Instant::now);
            if idle_timeout_s > 0 && since.elapsed() >= Duration::from_secs(idle_timeout_s) {
                return Ok(());
            }
        } else {
            idle_since = None;
        }

        let frame_start = Instant::now();
        let (mut config, loop_mode, output_encoding, max_fps) = session.snapshot();

        let prep_start = Instant::now();
        let init_image = match loop_mode {
            LoopMode::Feed => loop {
                cancel.check()?;
                if session.stop_requested() {
                    return Ok(());
                }
                if let Some(frame) = session.take_mailbox_frame() {
                    break Some(frame);
                }
                session.wait_for_mailbox(Duration::from_millis(250));
            },
            LoopMode::Feedback => {
                let base = last_output.take().or_else(|| session.take_mailbox_frame());
                base.map(|image| warp_feedback(&image, &config.camera))
            }
        };
        resolve_seed(&mut config, frame_index);
        let prep_ms = prep_start.elapsed().as_secs_f64() * 1000.0;

        cancel.check()?;
        let step = backend.live_step(
            LiveFrameIn {
                init: init_image.as_ref(),
                frame_index,
                config: &config,
            },
            cancel,
        );
        let out = match step {
            Ok(out) => out,
            Err(e) => {
                if !matches!(e, AssetAiError::Cancelled) {
                    session.push_bytes(realtime_wire::encode_error_message(&e.to_string()).into_bytes());
                }
                return Err(e);
            }
        };

        let post_start = Instant::now();
        let frame_bytes = encode_output_frame(&out.image, output_encoding, frame_index as u32);
        session.push_bytes(frame_bytes);
        let post_ms = post_start.elapsed().as_secs_f64() * 1000.0;
        last_output = Some(out.image);

        let frames_out = session.frames_out.fetch_add(1, Ordering::Relaxed) + 1;
        let frames_in = session.frames_in.load(Ordering::Relaxed);
        let dropped = session.dropped.load(Ordering::Relaxed);
        let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
        let fps = if frame_ms > 0.0 { 1000.0 / frame_ms } else { 0.0 };

        session.push_bytes(
            realtime_wire::encode_stats_message(realtime_wire::StatsMessageJson {
                kind: String::new(),
                frame_index,
                fps,
                frame_ms,
                stage_ms: realtime_wire::StageMsJson {
                    prep: prep_ms,
                    model: out.model_ms,
                    post: post_ms,
                },
                frames_in,
                frames_out,
                dropped,
            })
            .into_bytes(),
        );

        if last_progress_push.elapsed() >= Duration::from_millis(100) {
            progress("live", frames_in, frames_out, fps);
            last_progress_push = Instant::now();
        }

        if max_fps > 0.0 {
            let target = Duration::from_secs_f64(1.0 / max_fps);
            let elapsed = frame_start.elapsed();
            if elapsed < target {
                std::thread::sleep(target - elapsed);
            }
        }

        frame_index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::LiveConfig;
    use crate::realtime_wire::{CameraUpdateJson, ControlUpdateJson};

    fn blank_config() -> LiveConfig {
        LiveConfig::default()
    }

    #[test]
    fn control_merge_only_touches_given_fields() {
        let mut config = blank_config();
        config.prompt = "original".to_string();
        config.steps = 4;
        config.strength = 0.6;
        config.seed = 5;

        let update = ControlUpdateJson {
            kind: "control".to_string(),
            prompt: Some("new prompt".to_string()),
            steps: Some(20),
            ..Default::default()
        };
        apply_control_to_config(&mut config, &update);

        assert_eq!(config.prompt, "new prompt");
        assert_eq!(config.steps, 20);
        // Untouched fields keep their prior values.
        assert_eq!(config.strength, 0.6);
        assert_eq!(config.seed, 5);
        assert_eq!(config.negative_prompt, "");
    }

    #[test]
    fn control_merge_camera_is_also_partial() {
        let mut config = blank_config();
        config.camera.dolly = 0.2;
        config.camera.pan_x = 0.1;

        let update = ControlUpdateJson {
            kind: "control".to_string(),
            camera: Some(CameraUpdateJson {
                dolly: Some(0.9),
                pan_x: None,
                pan_y: None,
                roll: None,
            }),
            ..Default::default()
        };
        apply_control_to_config(&mut config, &update);

        assert_eq!(config.camera.dolly, 0.9);
        // pan_x was not present in the camera update -> unchanged.
        assert_eq!(config.camera.pan_x, 0.1);
    }

    #[test]
    fn control_merge_clamps_and_parses_enums() {
        let mut config = blank_config();
        let update = ControlUpdateJson {
            kind: "control".to_string(),
            strength: Some(5.0), // out of range, clamps to 1.0
            steps: Some(9999),   // clamps to 200
            seed_mode: Some("increment".to_string()),
            ..Default::default()
        };
        apply_control_to_config(&mut config, &update);
        assert_eq!(config.strength, 1.0);
        assert_eq!(config.steps, 200);
        assert_eq!(config.seed_mode, SeedMode::Increment);

        // An unknown seed_mode string is silently ignored (kept as-is) —
        // malformed control fields must not crash a live session.
        let mut config2 = blank_config();
        let bad = ControlUpdateJson {
            kind: "control".to_string(),
            seed_mode: Some("nonsense".to_string()),
            ..Default::default()
        };
        apply_control_to_config(&mut config2, &bad);
        assert_eq!(config2.seed_mode, SeedMode::Fixed);
    }

    #[test]
    fn warp_feedback_identity_at_zero_motion() {
        let image = RgbImage {
            width: 4,
            height: 4,
            data: (0..48).map(|i| (i * 5) as u8).collect(),
        };
        let warped = warp_feedback(&image, &CameraMotion::default());
        assert_eq!(warped.data, image.data);
    }

    #[test]
    fn warp_feedback_zoom_moves_a_known_pixel() {
        let width = 8u32;
        let height = 8u32;
        let mut data = vec![0u8; (width * height * 3) as usize];
        // A bright marker at the top-left corner only.
        data[0] = 255;
        data[1] = 255;
        data[2] = 255;
        let image = RgbImage { width, height, data };

        let camera = CameraMotion { dolly: 4.0, pan_x: 0.0, pan_y: 0.0, roll: 0.0 };
        let warped = warp_feedback(&image, &camera);
        // Zooming in samples closer to the center for every output pixel,
        // so the pure corner marker no longer reproduces exactly at (0,0).
        assert_ne!(&warped.data[0..3], &image.data[0..3]);
    }

    #[test]
    fn warp_feedback_zero_size_image_is_identity() {
        let image = RgbImage { width: 0, height: 0, data: Vec::new() };
        let warped = warp_feedback(&image, &CameraMotion { dolly: 1.0, ..Default::default() });
        assert_eq!(warped.data, image.data);
    }

    #[test]
    fn resolve_seed_modes() {
        let mut config = blank_config();
        config.seed = 100;
        config.seed_mode = SeedMode::Fixed;
        resolve_seed(&mut config, 7);
        assert_eq!(config.seed, 100);

        let mut config = blank_config();
        config.seed = 100;
        config.seed_mode = SeedMode::Increment;
        resolve_seed(&mut config, 7);
        assert_eq!(config.seed, 107);

        let mut config_a = blank_config();
        config_a.seed = 100;
        config_a.seed_mode = SeedMode::Random;
        resolve_seed(&mut config_a, 7);
        let mut config_b = blank_config();
        config_b.seed = 100;
        config_b.seed_mode = SeedMode::Random;
        resolve_seed(&mut config_b, 7);
        // Deterministic given the same base seed + frame index.
        assert_eq!(config_a.seed, config_b.seed);
        // And not simply the base seed or the frame index.
        assert_ne!(config_a.seed, 100);
        assert_ne!(config_a.seed, 7);
    }

    #[test]
    fn session_mailbox_keeps_only_latest_and_counts_dropped() {
        let params = LiveParams {
            model: "testpattern".to_string(),
            config: LiveConfig::default(),
            loop_mode: LoopMode::Feed,
            output_encoding: OutputEncoding::Raw,
            max_fps: 0.0,
            idle_timeout_s: 30,
        };
        let session = RealtimeSession::new("job-1".to_string(), &params);
        session.push_input_frame(RgbImage::blank(2, 2));
        session.push_input_frame(RgbImage::blank(3, 3)); // overwrites, unconsumed -> dropped
        assert_eq!(session.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(session.frames_in.load(Ordering::Relaxed), 2);
        let latest = session.take_mailbox_frame().unwrap();
        assert_eq!((latest.width, latest.height), (3, 3));
        assert!(session.take_mailbox_frame().is_none());
    }

    #[test]
    fn session_apply_control_updates_session_only_knobs() {
        let params = LiveParams {
            model: "testpattern".to_string(),
            config: LiveConfig::default(),
            loop_mode: LoopMode::Feed,
            output_encoding: OutputEncoding::Raw,
            max_fps: 0.0,
            idle_timeout_s: 30,
        };
        let session = RealtimeSession::new("job-1".to_string(), &params);
        let update = ControlUpdateJson {
            kind: "control".to_string(),
            loop_mode: Some("feedback".to_string()),
            output_encoding: Some("png".to_string()),
            max_fps: Some(24.0),
            prompt: Some("hi".to_string()),
            ..Default::default()
        };
        session.apply_control(&update);
        let (config, loop_mode, output_encoding, max_fps) = session.snapshot();
        assert_eq!(loop_mode, LoopMode::Feedback);
        assert_eq!(output_encoding, OutputEncoding::Png);
        assert_eq!(max_fps, 24.0);
        assert_eq!(config.prompt, "hi");
    }
}
