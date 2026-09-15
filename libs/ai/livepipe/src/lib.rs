//! A live picture pipe to the ai-hub fleet: SOURCE -> ENCODER -> SESSION
//! -> RESULTS, consumer-agnostic.
//!
//! - Source: the platform camera (a device picked by name) or a still fed
//!   at ~10 Hz (tests, dark rooms), both through one [`FrameMailbox`]: the
//!   sender asks for a frame, the source converts the next one, the sender
//!   takes it. Nothing is converted that nobody sends.
//! - Encoder: raw RGB8 or H.264 on the wire. ONE VideoToolbox encoder per
//!   run, made at the first frame's size — a session per frame is the churn
//!   that panics the Mac. An H.264 session the box never answers is reopened
//!   in raw, so a consumer's switch never breaks on the codec.
//! - Session: the hub's realtime session (POST /realtime + websocket,
//!   `loop_mode: feed`) on a node chosen by DOMAIN: an explicit
//!   `http://host:port`, else the first fleet node whose `/health` lists the
//!   domain. Two frames in flight at most; the next one is grabbed as soon
//!   as a result lands, so the box always has the following frame ready and
//!   the result is as fresh as the model is fast.
//! - Results: whatever the model puts on the `aux` channel, handed over as
//!   text with its frame index and the box's own ms; the consumer parses its
//!   packet (pose, text, boxes). Rate and status come as their own events.
//!
//! The compute is always on the box; nothing here runs a model locally.

use makepad_ai_hub::http_client::{http_fetch, HttpClientRequest};
use makepad_video::{StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::makepad_platform::makepad_network::plain_web_socket::PlainWebSocket;
use makepad_widgets::makepad_platform::video::{
    CameraFrameLayout, CameraFrameRef, VideoFormat, VideoFormatId, VideoInputId, VideoInputsEvent,
    VideoPixelFormat,
};
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The wire frame: `[magic u32][kind u8][0][0 u16][width u16][height u16]
/// [frame_index u32]` then the payload (`libs/ai/hub/src/realtime_wire.rs`).
pub const FRAME_MAGIC: u32 = 0x4C46_5246;
pub const FRAME_HEADER_LEN: usize = 16;
/// Frames are point-sampled down so the longest edge is at most this: a
/// person fills enough pixels for a body model and the wire stays cheap.
pub const SEND_MAX_WIDTH: usize = 640;
/// An answered session that goes quiet: after this the frame is written off
/// and the next one goes.
const IN_FLIGHT_TIMEOUT: Duration = Duration::from_secs(8);
/// The first packet loads the model on the box (a cold node takes minutes).
const FIRST_PACKET_TIMEOUT: Duration = Duration::from_secs(180);
const MIN_SEND_INTERVAL: Duration = Duration::from_millis(15);
const MAX_IN_FLIGHT: usize = 2;
/// Unanswered H.264 rounds before the session is reopened in raw RGB.
const H264_UNANSWERED_LIMIT: u32 = 2;

/// What the frames are on the wire. Raw is the default until the box's
/// decoder proves itself; the encoder cannot be missing here (raw) and an
/// unanswered H.264 session falls back to raw by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WireEncoding {
    H264,
    #[default]
    Raw,
}

impl WireEncoding {
    pub fn key(self) -> &'static str {
        match self {
            WireEncoding::H264 => "h264",
            WireEncoding::Raw => "raw",
        }
    }
    pub fn from_key(k: &str) -> WireEncoding {
        match k.trim() {
            "h264" => WireEncoding::H264,
            _ => WireEncoding::Raw,
        }
    }
    pub fn index(self) -> usize {
        match self {
            WireEncoding::H264 => 0,
            WireEncoding::Raw => 1,
        }
    }
}

/// Where the frames come from.
#[derive(Clone, Debug, PartialEq)]
pub enum PipeSource {
    /// The platform camera, routed in with [`install_camera`].
    Camera,
    /// A still fed at ~10 Hz: a CHW f32 3xNxN tensor (`.f32`) or a packed
    /// RGB8 file named `<name>-WxH.rgb`.
    Still(PathBuf),
}

/// One run's configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct PipeConfig {
    /// `http://host:port` of a hub node, or anything else to discover one
    /// that advertises `domain`.
    pub target: String,
    /// The capability the node must list in `/health` (`body`, `vision`).
    pub domain: String,
    /// The hub model the session asks for.
    pub model: String,
    /// The session's option words, sent as its prompt (`hands`,
    /// `detect persons=2`); empty = the model's plain pass.
    pub options: String,
    pub encoding: WireEncoding,
    pub source: PipeSource,
    pub max_fps: u32,
}

impl PipeConfig {
    pub fn new(domain: &str, model: &str) -> PipeConfig {
        PipeConfig {
            target: String::new(),
            domain: domain.to_string(),
            model: model.to_string(),
            options: String::new(),
            encoding: WireEncoding::default(),
            source: PipeSource::Camera,
            max_fps: 15,
        }
    }
}

/// Load a still as RGB8: a CHW f32 tensor (0..1, or ImageNet-normalised —
/// negative values — which is undone) or a packed `<name>-WxH.rgb`.
pub fn load_still(path: &Path) -> Option<(u16, u16, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    if path.extension().is_some_and(|e| e == "f32") {
        let n = bytes.len() / 4;
        let side = ((n / 3) as f64).sqrt() as usize;
        if side < 16 || side * side * 3 != n {
            return None;
        }
        let f = |i: usize| f32::from_le_bytes([bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]]);
        let mut lo = f32::MAX;
        for i in 0..n {
            lo = lo.min(f(i));
        }
        let normalised = lo < -0.01;
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        let plane = side * side;
        let mut rgb = Vec::with_capacity(plane * 3);
        for p in 0..plane {
            for c in 0..3 {
                let raw = f(c * plane + p);
                let v = if normalised { raw * std[c] + mean[c] } else { raw };
                rgb.push((v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        return Some((side as u16, side as u16, rgb));
    }
    let stem = path.file_stem()?.to_str()?;
    let dims = stem.rsplit('-').next()?;
    let (w, h) = dims.split_once('x')?;
    let (w, h): (usize, usize) = (w.parse().ok()?, h.parse().ok()?);
    (bytes.len() == w * h * 3).then(|| (w as u16, h as u16, bytes))
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

/// One RGB8 frame ready to send.
pub struct RgbFrame {
    pub width: u16,
    pub height: u16,
    pub rgb: Vec<u8>,
    pub taken: Instant,
}

/// The one-slot handoff from a source to the sender: the source only
/// converts a frame when `want` is raised (a YUV -> RGB pass at 30 fps for
/// frames nobody sends is heat), and the sender takes it.
#[derive(Clone, Default)]
pub struct FrameMailbox {
    want: Arc<AtomicBool>,
    slot: Arc<Mutex<Option<RgbFrame>>>,
    frames_seen: Arc<AtomicBool>,
}

impl FrameMailbox {
    pub fn request(&self) {
        self.want.store(true, Ordering::Relaxed);
    }

    pub fn take(&self) -> Option<RgbFrame> {
        self.slot.lock().unwrap().take()
    }

    /// Has the source delivered anything at all yet? Tells "no camera
    /// selected" from "the box is slow".
    pub fn live(&self) -> bool {
        self.frames_seen.load(Ordering::Relaxed)
    }

    /// A frame that did not come from the platform's camera (a still, a
    /// decoded file, a render target): same one-slot contract.
    pub fn offer_rgb(&self, width: u16, height: u16, rgb: Vec<u8>) {
        self.frames_seen.store(true, Ordering::Relaxed);
        if !self.want.load(Ordering::Relaxed) {
            return;
        }
        self.want.store(false, Ordering::Relaxed);
        *self.slot.lock().unwrap() = Some(RgbFrame { width, height, rgb, taken: Instant::now() });
    }

    fn offer(&self, frame: CameraFrameRef<'_>) {
        self.frames_seen.store(true, Ordering::Relaxed);
        if !self.want.load(Ordering::Relaxed) {
            return;
        }
        let Some(rgb) = frame_to_rgb(&frame) else { return };
        self.want.store(false, Ordering::Relaxed);
        *self.slot.lock().unwrap() = Some(rgb);
    }
}

/// Route the camera's frames into `mailbox`. Registering the callback is
/// also what makes the platform enumerate cameras, so `Event::VideoInputs`
/// follows; answer it with [`pick_camera_named`] + `cx.use_video_input`.
pub fn install_camera(cx: &mut Cx, mailbox: &FrameMailbox) {
    let mailbox = mailbox.clone();
    cx.camera_frame_input(0, move |frame| mailbox.offer(frame));
}

/// The first device, at the smallest NV12 (else YUY2) format that still
/// reaches 640x360.
pub fn pick_camera(ev: &VideoInputsEvent) -> Option<(VideoInputId, VideoFormatId, usize, usize)> {
    pick_camera_named(ev, "")
}

/// The same choice for a device picked by name; an unknown or empty name
/// falls back to the first device.
pub fn pick_camera_named(
    ev: &VideoInputsEvent,
    name: &str,
) -> Option<(VideoInputId, VideoFormatId, usize, usize)> {
    let desc = ev
        .descs
        .iter()
        .find(|d| !name.is_empty() && d.name == name)
        .or_else(|| ev.descs.first())?;
    let rank = |f: &VideoFormat| match f.pixel_format {
        VideoPixelFormat::NV12 => 2,
        VideoPixelFormat::YUY2 => 1,
        _ => 0,
    };
    let mut best: Option<&VideoFormat> = None;
    for f in &desc.formats {
        if rank(f) == 0 {
            continue;
        }
        let big_enough = f.width >= 640 && f.height >= 360;
        let better = match best {
            None => true,
            Some(b) => {
                let b_big = b.width >= 640 && b.height >= 360;
                if big_enough != b_big {
                    big_enough
                } else if rank(f) != rank(b) {
                    rank(f) > rank(b)
                } else if big_enough {
                    f.width * f.height < b.width * b.height
                } else {
                    f.width * f.height > b.width * b.height
                }
            }
        };
        if better {
            best = Some(f);
        }
    }
    let f = best?;
    Some((desc.input_id, f.format_id, f.width, f.height))
}

fn yuv_to_rgb(y: i32, u: i32, v: i32) -> [u8; 3] {
    let c = y - 16;
    let d = u - 128;
    let e = v - 128;
    let clip = |a: i32| a.clamp(0, 255) as u8;
    [
        clip((298 * c + 409 * e + 128) >> 8),
        clip((298 * c - 100 * d - 208 * e + 128) >> 8),
        clip((298 * c + 516 * d + 128) >> 8),
    ]
}

/// NV12 / YUY2 -> RGB8, point-sampled down by an integer factor so the
/// longest edge lands at or under [`SEND_MAX_WIDTH`]. Even output dims (the
/// wire's raw frame wants them).
pub fn frame_to_rgb(frame: &CameraFrameRef<'_>) -> Option<RgbFrame> {
    if frame.width == 0 || frame.height == 0 {
        return None;
    }
    let k = frame.width.div_ceil(SEND_MAX_WIDTH).max(1);
    let ow = (frame.width / k) & !1;
    let oh = (frame.height / k) & !1;
    if ow < 16 || oh < 16 {
        return None;
    }
    let mut rgb = Vec::with_capacity(ow * oh * 3);
    match frame.layout {
        CameraFrameLayout::NV12 => {
            let yp = &frame.planes[0];
            let uvp = &frame.planes[1];
            if yp.bytes.is_empty() || uvp.bytes.is_empty() {
                return None;
            }
            for oy in 0..oh {
                let sy = oy * k;
                let yrow = sy * yp.row_stride;
                let uvrow = (sy / 2) * uvp.row_stride;
                for ox in 0..ow {
                    let sx = ox * k;
                    let y = *yp.bytes.get(yrow + sx)? as i32;
                    let uvi = uvrow + (sx / 2) * 2;
                    let u = *uvp.bytes.get(uvi)? as i32;
                    let v = *uvp.bytes.get(uvi + 1)? as i32;
                    rgb.extend_from_slice(&yuv_to_rgb(y, u, v));
                }
            }
        }
        CameraFrameLayout::YUY2 => {
            let p = &frame.planes[0];
            if p.bytes.is_empty() {
                return None;
            }
            for oy in 0..oh {
                let row = (oy * k) * p.row_stride;
                for ox in 0..ow {
                    let sx = ox * k;
                    let y = *p.bytes.get(row + sx * 2)? as i32;
                    let pair = row + (sx & !1) * 2;
                    let u = *p.bytes.get(pair + 1)? as i32;
                    let v = *p.bytes.get(pair + 3)? as i32;
                    rgb.extend_from_slice(&yuv_to_rgb(y, u, v));
                }
            }
        }
        _ => return None,
    }
    Some(RgbFrame { width: ow as u16, height: oh as u16, rgb, taken: Instant::now() })
}

/// Feed a still into the mailbox at ~10 Hz for as long as `alive` holds.
fn spawn_still_source(path: PathBuf, mailbox: FrameMailbox, alive: Arc<AtomicBool>) -> bool {
    let Some((w, h, rgb)) = load_still(&path) else {
        return false;
    };
    std::thread::Builder::new()
        .name("livepipe-still".into())
        .spawn(move || {
            while alive.load(Ordering::Relaxed) {
                mailbox.offer_rgb(w, h, rgb.clone());
                std::thread::sleep(Duration::from_millis(100));
            }
        })
        .is_ok()
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// What a run reports.
pub enum PipeEvent {
    /// A line for a status box: where the run is.
    Status(String),
    /// The session is open on a box.
    Connected { base: String, gpu: String, ws_path: String },
    /// One result from the model's `aux` channel: the message text (the
    /// consumer parses its own packet), its frame index and the box's ms.
    Aux { frame_index: u32, ms: f32, text: String },
    /// Throughput: at the first result and every ~3 s after.
    Rate { ms: f32, fps: f32 },
    Ended,
}

enum Cmd {
    Stop,
}

/// A running pipe. Dropping it stops the box-side job.
pub struct PipeHandle {
    tx: Sender<Cmd>,
    events: Receiver<PipeEvent>,
    pub mailbox: FrameMailbox,
    pub config: PipeConfig,
}

impl PipeHandle {
    pub fn start(config: PipeConfig, mailbox: FrameMailbox) -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let (etx, events) = mpsc::channel::<PipeEvent>();
        let thread_mailbox = mailbox.clone();
        let thread_config = config.clone();
        std::thread::Builder::new()
            .name("livepipe".into())
            .spawn(move || run(thread_config, thread_mailbox, rx, etx))
            .ok();
        Self { tx, events, mailbox, config }
    }

    pub fn try_recv(&self) -> Option<PipeEvent> {
        match self.events.try_recv() {
            Ok(ev) => Some(ev),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(PipeEvent::Ended),
        }
    }

    /// Stop and wait (briefly) for the box to be told: the session thread
    /// sends `stop`, cancels the job and answers `Ended`. For shutdown,
    /// where a plain drop would race process exit and leave the box's live
    /// slot to its idle timeout.
    pub fn stop_blocking(self) {
        let _ = self.tx.send(Cmd::Stop);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match self.events.recv_timeout(Duration::from_millis(50)) {
                Ok(PipeEvent::Ended) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// A node's `/health`: does it serve `domain`, and what GPU is it.
pub fn node_health(base: &str, domain: &str) -> Option<(bool, String)> {
    let url = format!("{base}/health");
    let resp = http_fetch(&HttpClientRequest::get(&url)).ok()?;
    let bytes = resp.read_body_to_vec(64 * 1024).ok()?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let gpu = match JsonValue::deserialize_json(&text).ok()? {
        JsonValue::Object(obj) => match obj.get("gpu") {
            Some(JsonValue::String(g)) => g.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    };
    Some((text.contains(&format!("\"{domain}\"")), gpu))
}

/// The fleet nodes advertising `domain` right now, for a settings picker:
/// (base url, gpu). A short listen, then one health probe per node.
pub fn domain_nodes(domain: &str, listen: Duration) -> Vec<(String, String)> {
    let discovered = makepad_ai_hub::discovery::start_listener();
    std::thread::sleep(listen);
    let mut out = Vec::new();
    for node in discovered.nodes() {
        if let Some((true, gpu)) = node_health(&node.base_url, domain) {
            out.push((node.base_url, gpu));
        }
    }
    out.sort();
    out
}

/// `(base, gpu)` of the box to use.
fn resolve_box(target: &str, domain: &str, out: &Sender<PipeEvent>) -> Option<(String, String)> {
    let target = target.trim().trim_end_matches('/');
    if target.starts_with("http://") {
        let gpu = node_health(target, domain).map(|(_, g)| g).unwrap_or_default();
        return Some((target.to_string(), gpu));
    }
    let _ = out.send(PipeEvent::Status(format!("{domain}: looking for a hub node")));
    let discovered = makepad_ai_hub::discovery::start_listener();
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut tried: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        for node in discovered.nodes() {
            if tried.contains(&node.base_url) {
                continue;
            }
            tried.push(node.base_url.clone());
            if let Some((true, gpu)) = node_health(&node.base_url, domain) {
                return Some((node.base_url, gpu));
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = out.send(PipeEvent::Status(format!(
        "{domain}: no hub node advertises it ({} probed)",
        tried.len()
    )));
    None
}

fn open_session(base: &str, width: u16, height: u16, cfg: &PipeConfig) -> Result<String, String> {
    let model = &cfg.model;
    let options = cfg.options.trim();
    let prompt = if options.is_empty() {
        String::new()
    } else {
        format!(",\"prompt\":\"{}\"", options.replace('\\', "").replace('"', ""))
    };
    let encoding = cfg.encoding.key();
    let max_fps = cfg.max_fps.max(1);
    let body = format!(
        "{{\"model\":\"{model}\",\"width\":{width},\"height\":{height},\"input_encoding\":\"{encoding}\",\"output_encoding\":\"none\",\"loop_mode\":\"feed\",\"max_fps\":{max_fps},\"idle_timeout_s\":60,\"queue_policy\":\"queue\"{prompt}}}"
    );
    let url = format!("{base}/realtime");
    let response = http_fetch(&HttpClientRequest::post(&url, "application/json", body.as_bytes()))
        .map_err(|e| format!("realtime open: {e:?}"))?;
    let status = response.status;
    let bytes = response
        .read_body_to_vec(1024 * 1024)
        .map_err(|e| format!("realtime open: {e:?}"))?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    if status != 200 {
        return Err(format!(
            "realtime open: http {status} {}",
            text.chars().take(160).collect::<String>()
        ));
    }
    let root = JsonValue::deserialize_json(&text).map_err(|e| format!("realtime open: {e:?}"))?;
    let JsonValue::Object(obj) = root else {
        return Err("realtime open: not an object".into());
    };
    match obj.get("ws_path") {
        Some(JsonValue::String(p)) => Ok(p.clone()),
        _ => Err(format!("realtime open: no ws_path in {text}")),
    }
}

/// Cancel the session's job on the box. A session left live queues every
/// later one behind it (observed 2026-09-02: three sessions, zero frames),
/// so the cancel is logged either way.
pub fn cancel_job(base: &str, ws_path: &str) {
    if let Some(job) = ws_path.rsplit('/').next().filter(|s| !s.is_empty()) {
        let ok = http_fetch(&HttpClientRequest::post(
            &format!("{base}/job/{job}/cancel"),
            "application/json",
            b"{}",
        ))
        .is_ok();
        log!("livepipe: cancelled {job} on {base}: {}", if ok { "ok" } else { "no answer" });
    }
}

// ---------------------------------------------------------------------------
// Wire
// ---------------------------------------------------------------------------

pub fn encode_raw_frame(frame: &RgbFrame, index: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + frame.rgb.len());
    out.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
    out.push(0); // raw RGB8
    out.push(0);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&index.to_le_bytes());
    out.extend_from_slice(&frame.rgb);
    out
}

/// One H.264 access unit as a wire frame (kind 2). The node's decoder
/// reads the size from the SPS, so width/height ride along for the log.
pub fn encode_h264_frame(width: u16, height: u16, nal: &[u8], index: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + nal.len());
    out.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
    out.push(2); // H.264
    out.push(0);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&index.to_le_bytes());
    out.extend_from_slice(nal);
    out
}

/// The per-run encoder: made ONCE at the first frame's size (a VideoToolbox
/// session per frame is the churn that panics the Mac).
pub struct WireEncoder {
    enc: VideoStreamEncoder,
    width: u16,
    height: u16,
    fps: u32,
    pts: i64,
}

impl WireEncoder {
    pub fn open(width: u16, height: u16, fps: u32) -> Result<WireEncoder, String> {
        let fps = fps.max(1);
        let enc = VideoStreamEncoder::new(VideoStreamEncoderOptions {
            codec: StreamVideoCodec::H264,
            width: width as u32 & !1,
            height: height as u32 & !1,
            fps,
            bitrate_kbps: 2500,
            keyint: 30,
            low_latency: true,
        })
        .map_err(|e| format!("{e:?}"))?;
        Ok(WireEncoder { enc, width, height, fps, pts: 0 })
    }

    /// The wire frames for one RGB frame (usually one; none when the
    /// encoder buffers). A frame of another size than the session's is
    /// dropped rather than re-opening the session.
    pub fn encode(&mut self, frame: &RgbFrame, index: u32) -> Vec<Vec<u8>> {
        if frame.width != self.width || frame.height != self.height {
            return Vec::new();
        }
        self.pts += 10_000_000 / self.fps as i64;
        match self.enc.push_frame_rgb8(&frame.rgb, self.pts) {
            Ok(packets) => packets
                .into_iter()
                .map(|p| encode_h264_frame(self.width, self.height, &p.data, index))
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

pub fn is_frame_message(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) == FRAME_MAGIC
}

fn num(v: &JsonValue) -> Option<f32> {
    match v {
        JsonValue::F64(f) => Some(*f as f32),
        JsonValue::I64(n) => Some(*n as f32),
        JsonValue::U64(n) => Some(*n as f32),
        _ => None,
    }
}

/// `{"type":"aux","frame_index":N,"data":{"ms":..}}` -> (frame index, ms);
/// `None` for any other message.
fn aux_meta(text: &str) -> Option<(u32, f32)> {
    let JsonValue::Object(obj) = JsonValue::deserialize_json(text).ok()? else {
        return None;
    };
    match obj.get("type") {
        Some(JsonValue::String(t)) if t == "aux" => {}
        _ => return None,
    }
    let frame_index = obj.get("frame_index").and_then(num).unwrap_or(0.0) as u32;
    let ms = match obj.get("data") {
        Some(JsonValue::Object(data)) => data.get("ms").and_then(num).unwrap_or(0.0),
        _ => 0.0,
    };
    Some((frame_index, ms))
}

fn run(cfg: PipeConfig, mailbox: FrameMailbox, rx: Receiver<Cmd>, out: Sender<PipeEvent>) {
    let tag = cfg.domain.clone();
    let alive = Arc::new(AtomicBool::new(true));
    // Drop guard: whatever way this returns, the still feeder stops.
    struct Alive(Arc<AtomicBool>);
    impl Drop for Alive {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Relaxed);
        }
    }
    let _alive = Alive(alive.clone());
    if let PipeSource::Still(path) = &cfg.source {
        if spawn_still_source(path.clone(), mailbox.clone(), alive.clone()) {
            let _ = out.send(PipeEvent::Status(format!("{tag}: feeding the still {}", path.display())));
        } else {
            let _ = out.send(PipeEvent::Status(format!("{tag}: cannot read the still {}", path.display())));
            let _ = out.send(PipeEvent::Ended);
            return;
        }
    }
    let Some((base, gpu)) = resolve_box(&cfg.target, &cfg.domain, &out) else {
        let _ = out.send(PipeEvent::Ended);
        return;
    };
    // The first frame decides the session size; wait for it here rather
    // than opening a session the source may never feed.
    let _ = out.send(PipeEvent::Status(format!("{tag}: {base} — waiting for the camera")));
    mailbox.request();
    let first = loop {
        if let Some(frame) = mailbox.take() {
            break frame;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Cmd::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = out.send(PipeEvent::Ended);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    let _ = out.send(PipeEvent::Status(format!(
        "{tag}: camera {}x{}, opening {base}",
        first.width, first.height
    )));
    // The wire encoder is decided BEFORE the session so the session is
    // opened with the encoding the frames will actually carry.
    let mut cfg = cfg;
    let mut encoder: Option<WireEncoder> = None;
    if cfg.encoding == WireEncoding::H264 {
        match WireEncoder::open(first.width, first.height, cfg.max_fps) {
            Ok(enc) => encoder = Some(enc),
            Err(e) => {
                log!("livepipe: H.264 encoder unavailable ({e}); sending raw RGB instead");
                let _ = out.send(PipeEvent::Status(format!("{tag}: no H.264 encoder here — raw RGB on the wire")));
                cfg.encoding = WireEncoding::Raw;
            }
        }
    }
    let (cam_w, cam_h) = (first.width, first.height);
    let mut ws_path = match open_session(&base, cam_w, cam_h, &cfg) {
        Ok(p) => p,
        Err(e) => {
            let _ = out.send(PipeEvent::Status(format!("{tag}: {base} refused — {e}")));
            let _ = out.send(PipeEvent::Ended);
            return;
        }
    };
    let ws_url = format!("{}{}", base.replacen("http://", "ws://", 1), ws_path);
    let (ws_tx, mut ws_rx) = mpsc::channel::<WebSocketMessage>();
    let mut socket = PlainWebSocket::open(LiveId::empty(), HttpRequest::new(ws_url, HttpMethod::GET), ws_tx);
    let _ = out.send(PipeEvent::Connected { base: base.clone(), gpu: gpu.clone(), ws_path: ws_path.clone() });
    let _ = out.send(PipeEvent::Status(format!(
        "{tag}: {base} session {ws_path}; the first packet loads the model on the box"
    )));
    log!(
        "livepipe: {base} session {ws_path} ({cam_w}x{cam_h} {}, model {}, options '{}')",
        cfg.encoding.key(),
        cfg.model,
        cfg.options
    );

    let mut index = 0u32;
    // Send times of the unanswered frames, oldest first.
    let mut in_flight: Vec<Instant> = Vec::new();
    let mut packets = 0u64;
    let mut last_send = Instant::now() - Duration::from_secs(1);
    let mut pending: Option<RgbFrame> = Some(first);
    let mut stats_mark = Instant::now();
    let mut stats_packets = 0u32;
    let mut h264_unanswered = 0u32;
    loop {
        match rx.recv_timeout(Duration::from_millis(5)) {
            Ok(Cmd::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = socket.send_message(WebSocketMessage::String("{\"type\":\"stop\"}".into()));
                cancel_job(&base, &ws_path);
                socket.close();
                let _ = out.send(PipeEvent::Ended);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        while let Ok(msg) = ws_rx.try_recv() {
            match msg {
                WebSocketMessage::Binary(data) => {
                    if is_frame_message(&data) {
                        continue;
                    }
                    let Ok(text) = String::from_utf8(data) else { continue };
                    if let Some((frame_index, ms)) = aux_meta(&text) {
                        if !in_flight.is_empty() {
                            in_flight.remove(0);
                        }
                        packets += 1;
                        stats_packets += 1;
                        mailbox.request();
                        if packets == 1 {
                            let _ = out.send(PipeEvent::Rate { ms, fps: 0.0 });
                        }
                        if stats_mark.elapsed() >= Duration::from_secs(3) {
                            let fps = stats_packets as f32 / stats_mark.elapsed().as_secs_f32();
                            let _ = out.send(PipeEvent::Rate { ms, fps });
                            stats_mark = Instant::now();
                            stats_packets = 0;
                        }
                        let _ = out.send(PipeEvent::Aux { frame_index, ms, text });
                    } else if text.contains("\"type\":\"stats\"") {
                        // The box's own view: frames in/out and the codec's
                        // dropped_decode count — the line that tells "the
                        // wire is fine" from "it cannot decode what we send".
                        if stats_mark.elapsed() >= Duration::from_secs(3) || packets == 0 {
                            log!("livepipe: box stats {}", text.chars().take(240).collect::<String>());
                        }
                    } else if text.contains("\"error\"") || text.contains("\"stopped\"") {
                        log!("livepipe: session ended: {}", text.chars().take(200).collect::<String>());
                        let _ = out.send(PipeEvent::Status(format!(
                            "{tag}: {}",
                            text.chars().take(120).collect::<String>()
                        )));
                        let _ = out.send(PipeEvent::Ended);
                        return;
                    }
                }
                WebSocketMessage::String(text) => {
                    if text.contains("\"error\"") || text.contains("\"stopped\"") {
                        log!("livepipe: session ended: {}", text.chars().take(200).collect::<String>());
                        let _ = out.send(PipeEvent::Ended);
                        return;
                    }
                }
                WebSocketMessage::Closed | WebSocketMessage::Error(_) => {
                    let _ = out.send(PipeEvent::Status(format!("{tag}: {base} closed the session")));
                    cancel_job(&base, &ws_path);
                    let _ = out.send(PipeEvent::Ended);
                    return;
                }
                WebSocketMessage::Opened => {}
            }
        }
        if pending.is_none() {
            pending = mailbox.take();
        }
        // A cold H.264 decoder may swallow its first access unit (an init
        // boundary produces no frame), so the wire keeps feeding it instead
        // of waiting 180 s on one frame; raw keeps the long first wait for
        // the model load.
        let timeout = if packets == 0 && encoder.is_none() { FIRST_PACKET_TIMEOUT } else { IN_FLIGHT_TIMEOUT };
        if in_flight.first().is_some_and(|t| t.elapsed() > timeout) {
            log!("livepipe: frame unanswered after {:?}; sending the next", timeout);
            in_flight.clear();
            mailbox.request();
            // An H.264 session the box never answers (its decoder yields
            // nothing) is reopened in raw RGB instead of waiting on the
            // codec: the consumer's switch works whatever the wire does.
            if encoder.is_some() && packets == 0 {
                h264_unanswered += 1;
                if h264_unanswered >= H264_UNANSWERED_LIMIT {
                    log!("livepipe: {base} answered none of {index} H.264 access units; reopening the session in raw RGB");
                    let _ = out.send(PipeEvent::Status(format!(
                        "{tag}: the box decoded no H.264 — raw RGB on the wire"
                    )));
                    let _ = socket.send_message(WebSocketMessage::String("{\"type\":\"stop\"}".into()));
                    cancel_job(&base, &ws_path);
                    socket.close();
                    encoder = None;
                    cfg.encoding = WireEncoding::Raw;
                    ws_path = match open_session(&base, cam_w, cam_h, &cfg) {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = out.send(PipeEvent::Status(format!("{tag}: {base} refused — {e}")));
                            let _ = out.send(PipeEvent::Ended);
                            return;
                        }
                    };
                    let ws_url = format!("{}{}", base.replacen("http://", "ws://", 1), ws_path);
                    let (tx, rx) = mpsc::channel::<WebSocketMessage>();
                    ws_rx = rx;
                    socket = PlainWebSocket::open(LiveId::empty(), HttpRequest::new(ws_url, HttpMethod::GET), tx);
                    let _ = out.send(PipeEvent::Connected {
                        base: base.clone(),
                        gpu: gpu.clone(),
                        ws_path: ws_path.clone(),
                    });
                    log!("livepipe: {base} session {ws_path} ({cam_w}x{cam_h} raw)");
                    index = 0;
                    pending = None;
                    h264_unanswered = 0;
                }
            }
        }
        // Until the first packet proves the session, one raw frame at a
        // time; an H.264 session gets a short warm-up burst (the decoder's
        // init boundary eats a packet) before the same steady state.
        let limit = if packets == 0 {
            if encoder.is_some() { 3 } else { 1 }
        } else {
            MAX_IN_FLIGHT
        };
        if in_flight.len() < limit && last_send.elapsed() >= MIN_SEND_INTERVAL {
            if let Some(frame) = pending.take() {
                // A frame that sat in the slot through a long inference is
                // history; ask for a fresh one instead of sending it.
                if frame.taken.elapsed() > Duration::from_millis(400) && packets > 0 {
                    mailbox.request();
                    continue;
                }
                // LIVEPIPE_DUMP=<dir>: every 20th sent frame as PNG, to see
                // what the box sees (auto-framing cameras crop).
                if index % 20 == 0 {
                    if let Ok(dir) = std::env::var("LIVEPIPE_DUMP") {
                        if let Ok(png) = makepad_ai_hub::testpattern::encode_png_rgb8(
                            &frame.rgb,
                            frame.width as usize,
                            frame.height as usize,
                        ) {
                            let path = format!("{dir}/livepipe-sent-{index:05}.png");
                            let _ = std::fs::write(&path, png);
                            log!("livepipe: dumped {path}");
                        }
                    }
                }
                let messages: Vec<Vec<u8>> = match encoder.as_mut() {
                    Some(enc) => enc.encode(&frame, index),
                    None => vec![encode_raw_frame(&frame, index)],
                };
                if messages.is_empty() {
                    // The encoder buffered (or refused a size change): the
                    // frame is not on the wire, so it is not in flight.
                    mailbox.request();
                    continue;
                }
                for bytes in messages {
                    if index < 3 {
                        log!("livepipe: sent frame #{index}: kind {} ({} bytes)", bytes[4], bytes.len());
                    }
                    if socket.send_message(WebSocketMessage::Binary(bytes)).is_err() {
                        let _ = out.send(PipeEvent::Status(format!("{tag}: send failed")));
                        cancel_job(&base, &ws_path);
                        let _ = out.send(PipeEvent::Ended);
                        return;
                    }
                }
                index = index.wrapping_add(1);
                in_flight.push(Instant::now());
                last_send = Instant::now();
                if in_flight.len() < limit {
                    mailbox.request();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The oracle's input crop under the checkout (skipped when absent).
    fn fixture() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/agent_state/sam3dbody/oracle/batch_img.f32");
        p.exists().then_some(p)
    }

    #[test]
    fn an_h264_wire_frame_carries_the_kind_2_header_and_the_nal_verbatim() {
        let nal = [0u8, 0, 0, 1, 0x67, 1, 2, 3];
        let bytes = encode_h264_frame(640, 360, &nal, 7);
        assert_eq!(bytes.len(), FRAME_HEADER_LEN + nal.len());
        assert_eq!(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]), FRAME_MAGIC);
        assert_eq!(bytes[4], 2, "kind 2 = H.264");
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 640);
        assert_eq!(u16::from_le_bytes([bytes[10], bytes[11]]), 360);
        assert_eq!(u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]), 7);
        assert_eq!(&bytes[16..], &nal);
        assert!(is_frame_message(&bytes));
        let raw = encode_raw_frame(&RgbFrame { width: 2, height: 2, rgb: vec![9; 12], taken: Instant::now() }, 1);
        assert_eq!(raw[4], 0, "kind 0 = raw RGB8");
        assert_eq!(raw.len(), FRAME_HEADER_LEN + 12);
    }

    #[test]
    fn the_aux_meta_reads_the_frame_index_and_the_box_ms() {
        let meta = aux_meta(r#"{"type":"aux","frame_index":41,"data":{"n_people":1,"ms":52.5,"people":[]}}"#);
        assert_eq!(meta, Some((41, 52.5)));
        assert_eq!(aux_meta(r#"{"type":"stats","frames_in":3}"#), None);
        assert_eq!(aux_meta("not json"), None);
    }

    #[test]
    fn the_fixture_still_decodes_to_a_person_sized_rgb_image() {
        let Some(path) = fixture() else { return };
        let (w, h, rgb) = load_still(&path).expect("decodes");
        assert_eq!((w, h), (512, 512));
        assert_eq!(rgb.len(), 512 * 512 * 3);
        let (lo, hi) = rgb.iter().fold((255u8, 0u8), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(hi as i32 - lo as i32 > 100, "a picture, not a flat tensor: {lo}..{hi}");
    }

    #[test]
    fn a_still_source_feeds_the_mailbox_on_request() {
        let Some(path) = fixture() else { return };
        let mailbox = FrameMailbox::default();
        let alive = Arc::new(AtomicBool::new(true));
        assert!(spawn_still_source(path, mailbox.clone(), alive.clone()));
        mailbox.request();
        let deadline = Instant::now() + Duration::from_secs(3);
        let frame = loop {
            if let Some(f) = mailbox.take() {
                break f;
            }
            assert!(Instant::now() < deadline, "the still never arrived");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!((frame.width, frame.height), (512, 512));
        assert!(mailbox.live());
        alive.store(false, Ordering::Relaxed);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_wire_encoder_turns_the_still_into_a_keyframe_first() {
        let Some(path) = fixture() else { return };
        let (w, h, rgb) = load_still(&path).expect("decodes");
        let mut enc = WireEncoder::open(w, h, 15).expect("a VideoToolbox encoder");
        let mut packets = Vec::new();
        for i in 0..4 {
            let frame = RgbFrame { width: w, height: h, rgb: rgb.clone(), taken: Instant::now() };
            packets.extend(enc.encode(&frame, i));
        }
        assert!(!packets.is_empty(), "four frames in, at least one access unit out");
        let first = &packets[0];
        assert_eq!(first[4], 2);
        let payload = &first[FRAME_HEADER_LEN..];
        let has_sps = payload.windows(5).any(|w| w[..4] == [0, 0, 0, 1] && w[4] & 0x1f == 7);
        assert!(has_sps, "the first access unit carries the SPS");
    }
}
