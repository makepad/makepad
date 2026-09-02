//! Live/realtime session: a permanent streaming job that owns the worker
//! (one GPU = one job) until stopped. `RealtimeSession` is the state shared
//! between the worker thread (running [`run_live`]) and the HTTP route
//! thread (`server::route_loop`, which feeds it control updates, reference
//! images and input frames from any number of connected websockets, and
//! registers/removes their output senders). See `protocol.rs`'s "Realtime
//! session wire protocol" doc block for the wire contract and
//! `realtime_wire.rs` for the (de)serialization helpers this module calls.

use crate::backend::{
    merge_feedback_fields, BorderMode, CameraMotion, CancelToken, ContentBackend, DriftParams,
    LiveConfig, LiveFrameIn, LiveParams, LoopMode, NoiseMode, OutputEncoding, RgbImage, SeedMode,
};
use crate::error::AssetAiError;
use crate::realtime_wire::{self, ClientMessage, FrameHeader, FrameKind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// H.264 input/output codec seam (`makepad-video`'s hardware
/// `VideoStreamEncoder`/`VideoStreamDecoder`) — only compiled with the
/// `video` cargo feature; see [`OutputEncoding::H264`]'s doc for what a
/// build without it does instead (refuses the request at admission time).
#[cfg(feature = "video")]
mod h264 {
    pub use makepad_video::{
        stream_debug, StreamVideoCodec, VideoStreamDecoder, VideoStreamEncoder, VideoStreamEncoderOptions,
    };
}

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
    input_encoding: OutputEncoding,
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
    /// H.264 input packets that failed to decode (see `handle_binary`) —
    /// surfaced as `stats.codec.dropped_decode`.
    dropped_decode: AtomicU64,
    /// Milliseconds the most recent completed outbound encode took, written
    /// by the encoder thread; the loop reports it as `stage_ms.post`.
    last_encode_ms: AtomicU64,
    /// Outbound frames overwritten before the encoder thread picked them up
    /// (the loop never waits for the encode).
    dropped_encode: AtomicU64,
    stop_requested: AtomicBool,
    /// `{"type":"control","reset":true}`: consumed once by the worker, which
    /// drops its previous output so the next feedback frame cold-starts.
    reset_requested: AtomicBool,
    /// Bumped every time reference slot 0 is set, so the worker can notice a
    /// new feedback source without diffing images.
    reference0_version: AtomicU64,
    /// `{"type":"seed_output"}`: the trip a feed made on ANOTHER box, handed
    /// to this session so it carries on instead of cold-starting. Taken once
    /// by the worker, which drops it into its previous-output slot.
    pending_seed: Mutex<Option<RgbImage>>,
    /// Persists across input packets (a streaming H.264 decoder needs SPS/
    /// PPS + reference-frame continuity). `handle_binary` runs on the HTTP
    /// route thread, so this — unlike the encoder below — must be shared,
    /// not local to `run_live`.
    #[cfg(feature = "video")]
    input_decoder: Mutex<Option<h264::VideoStreamDecoder>>,
    /// Persists across output frames (GOP/keyframe cadence). Only ever
    /// touched by the worker thread inside `run_live`, but lives here
    /// (rather than as a local in `run_live`) so `add_socket` can request a
    /// fresh keyframe for a newly joined socket without threading extra
    /// state through the worker loop.
    #[cfg(feature = "video")]
    output_encoder: Mutex<Option<h264::VideoStreamEncoder>>,
}

impl RealtimeSession {
    pub fn new(job_id: String, params: &LiveParams) -> Self {
        Self {
            job_id,
            model_id: params.model.clone(),
            state: Mutex::new(SessionState {
                config: params.config.clone(),
                loop_mode: params.loop_mode,
                input_encoding: params.input_encoding,
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
            dropped_decode: AtomicU64::new(0),
            last_encode_ms: AtomicU64::new(0),
            dropped_encode: AtomicU64::new(0),
            stop_requested: AtomicBool::new(false),
            reset_requested: AtomicBool::new(false),
            reference0_version: AtomicU64::new(0),
            pending_seed: Mutex::new(None),
            #[cfg(feature = "video")]
            input_decoder: Mutex::new(None),
            #[cfg(feature = "video")]
            output_encoder: Mutex::new(None),
        }
    }

    pub fn add_socket(&self, id: u64, sender: mpsc::Sender<Vec<u8>>) {
        self.sockets.lock().unwrap().push(SessionSocket { id, sender });
        // A feedback worker with no listener holds its loop on the mailbox
        // condvar; a consumer arriving is what it is waiting for.
        self.mailbox_cv.notify_all();
        // A fresh socket has no decoder state at all — get it a keyframe
        // now instead of making it wait for the encoder's own GOP cadence.
        #[cfg(feature = "video")]
        if let Ok(mut encoder) = self.output_encoder.lock() {
            if let Some(encoder) = encoder.as_mut() {
                encoder.request_keyframe();
            }
        }
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

    fn take_reset(&self) -> bool {
        self.reset_requested.swap(false, Ordering::Relaxed)
    }

    /// The trip carried in from another box, if one arrived since the last
    /// iteration. Taken once: from here on it is simply this session's
    /// previous output and the loop goes on from it.
    pub fn set_seed_output(&self, image: RgbImage) {
        if image.width == 0 || image.height == 0 || image.data.len() != image.width as usize * image.height as usize * 3 {
            return;
        }
        *self.pending_seed.lock().unwrap() = Some(image);
        // A feedback worker parked on the mailbox has something to do now.
        self.mailbox_cv.notify_all();
    }

    pub fn take_seed_output(&self) -> Option<RgbImage> {
        self.pending_seed.lock().unwrap().take()
    }

    /// Reference slot 0 if it was (re)set since `seen_version` — the
    /// feedback source a reference-message client provides. A 1x1 blank is
    /// the placeholder `set_reference` pads with, never a real source.
    fn take_new_reference0(&self, seen_version: &mut u64) -> Option<RgbImage> {
        let version = self.reference0_version.load(Ordering::Relaxed);
        if version == *seen_version {
            return None;
        }
        *seen_version = version;
        let state = self.state.lock().unwrap();
        state
            .config
            .references
            .first()
            .filter(|image| image.width > 1 || image.height > 1)
            .cloned()
    }

    fn wait_for_mailbox(&self, timeout: Duration) {
        let guard = self.mailbox.lock().unwrap();
        let _ = self.mailbox_cv.wait_timeout(guard, timeout);
    }

    fn idle_timeout_s(&self) -> u64 {
        self.state.lock().unwrap().idle_timeout_s
    }

    fn loop_mode(&self) -> LoopMode {
        self.state.lock().unwrap().loop_mode
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

    fn codec_stats(&self) -> realtime_wire::CodecStatsJson {
        let state = self.state.lock().unwrap();
        realtime_wire::CodecStatsJson {
            input: state.input_encoding.as_str().to_string(),
            output: state.output_encoding.as_str().to_string(),
            dropped_decode: self.dropped_decode.load(Ordering::Relaxed),
        }
    }

    /// Merges a partial `{"type":"control", ...}` update: only the fields
    /// present in `update` change anything (see [`apply_control_to_config`]
    /// for the `LiveConfig` subset; the session-only knobs are merged here).
    pub fn apply_control(
        &self,
        update: &realtime_wire::ControlUpdateJson,
    ) -> Result<(), AssetAiError> {
        let mut state = self.state.lock().unwrap();
        let next_loop_mode = update
            .loop_mode
            .as_deref()
            .and_then(|text| LoopMode::parse(text).ok())
            .unwrap_or(state.loop_mode);
        let next_output_encoding = update
            .output_encoding
            .as_deref()
            .and_then(|text| OutputEncoding::parse(text).ok())
            .filter(OutputEncoding::is_supported_in_this_build)
            .unwrap_or(state.output_encoding);
        if next_loop_mode == LoopMode::Feedback
            && next_output_encoding == OutputEncoding::None
        {
            return Err(AssetAiError::Params(
                "realtime: loop_mode \"feedback\" requires output frames; output_encoding \"none\" is not allowed"
                    .to_string(),
            ));
        }
        apply_control_to_config(&mut state.config, update);
        if let Some(mode) = update
            .loop_mode
            .as_deref()
            .and_then(|text| LoopMode::parse(text).ok())
        {
            let changed = state.loop_mode != mode;
            state.loop_mode = mode;
            if changed {
                // The server-loop handshake flips feed -> feedback right
                // after the first output, while the worker is parked on the
                // mailbox waiting for input that will never come. Wake it so
                // the flip takes effect now, not at the next timeout tick.
                self.mailbox_cv.notify_all();
            }
        }
        if let Some(encoding) = update
            .input_encoding
            .as_deref()
            .and_then(|text| OutputEncoding::parse(text).ok())
            .filter(OutputEncoding::is_supported_in_this_build)
        {
            state.input_encoding = encoding;
        }
        if let Some(encoding) = update
            .output_encoding
            .as_deref()
            .and_then(|text| OutputEncoding::parse(text).ok())
            .filter(OutputEncoding::is_supported_in_this_build)
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
        if update.reset == Some(true) {
            self.reset_requested.store(true, Ordering::Relaxed);
        }
        Ok(())
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
        if slot == 0 {
            self.reference0_version.fetch_add(1, Ordering::Relaxed);
            // A feedback worker waiting for its first source sleeps on the
            // mailbox condvar; slot 0 is a source too, so wake it.
            self.mailbox_cv.notify_all();
        }
    }

    /// Handles one client -> server binary message: decodes it as an input
    /// frame and pushes it to the mailbox. A malformed WIRE HEADER (bad
    /// magic, bad raw-frame length) is a protocol error and propagates. An
    /// H.264 packet that fails to DECODE is different — real network jitter
    /// can corrupt/drop packets — so that is swallowed here: counted in
    /// `dropped_decode` and reported back only via `stats`, never as a hard
    /// per-message error to the client.
    pub fn handle_binary(&self, bytes: &[u8]) -> Result<(), AssetAiError> {
        let (header, payload) = realtime_wire::decode_frame(bytes)?;
        match header.kind {
            FrameKind::Raw | FrameKind::Png => {
                let image = decode_frame_payload(header, payload)?;
                self.push_input_frame(image);
            }
            FrameKind::H264 => self.handle_binary_h264(header, payload),
        }
        Ok(())
    }

    #[cfg(feature = "video")]
    fn handle_binary_h264(&self, header: FrameHeader, payload: &[u8]) {
        let mut decoder_slot = self.input_decoder.lock().unwrap();
        if decoder_slot.is_none() {
            match h264::VideoStreamDecoder::new(h264::StreamVideoCodec::H264) {
                Ok(decoder) => *decoder_slot = Some(decoder),
                Err(e) => {
                    eprintln!("realtime: h264 input decoder init failed: {e}");
                    self.dropped_decode.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }
        let decoder = decoder_slot.as_mut().unwrap();
        // The decoder wants monotonic 100 ns timestamps; the wire only
        // carries a frame index, so stamp it at a nominal 30 fps.
        let pts_100ns = header.frame_index as i64 * 333_333;
        h264::stream_debug::log(|| {
            format!(
                "realtime: h264 frame #{} {} bytes head [{}]",
                header.frame_index,
                payload.len(),
                h264::stream_debug::head(payload)
            )
        });
        match decoder.push_packet(payload, pts_100ns) {
            Ok(frames) => {
                h264::stream_debug::log(|| format!("realtime: h264 frame #{} -> {} decoded", header.frame_index, frames.len()));
                if let Some(frame) = frames.into_iter().last() {
                    self.push_input_frame(RgbImage {
                        width: frame.width,
                        height: frame.height,
                        data: frame.to_rgb8(),
                    });
                }
                // Zero frames is a legitimate outcome (SPS/PPS-only packet,
                // or the decoder still buffering) — not a drop.
            }
            Err(e) => {
                let dropped = self.dropped_decode.fetch_add(1, Ordering::Relaxed) + 1;
                h264::stream_debug::log(|| format!("realtime: h264 frame #{} decode failed: {e}", header.frame_index));
                if dropped <= 3 || dropped % 100 == 0 {
                    eprintln!("realtime: h264 input decode failed, dropping packet ({dropped} so far): {e}");
                }
            }
        }
    }

    #[cfg(not(feature = "video"))]
    fn handle_binary_h264(&self, _header: FrameHeader, _payload: &[u8]) {
        self.dropped_decode.fetch_add(1, Ordering::Relaxed);
        eprintln!("realtime: received an h264 input frame but this build has no 'video' feature — dropped");
    }

    /// Handles one client -> server text message: control / reference / stop.
    pub fn handle_text(&self, text: &str) -> Result<(), AssetAiError> {
        match realtime_wire::parse_client_message(text)? {
            ClientMessage::Control(update) => self.apply_control(&update)?,
            ClientMessage::Reference(reference) => {
                let slot = reference.slot.unwrap_or(0) as usize;
                let png_b64 = reference.png_b64.as_deref().unwrap_or("");
                let bytes = makepad_base64::base64_decode(png_b64.as_bytes())
                    .map_err(|e| AssetAiError::Params(format!("reference: bad base64: {e:?}")))?;
                let (data, width, height) = crate::testpattern::decode_png_rgb8(&bytes)?;
                self.set_reference(slot, RgbImage { width, height, data });
            }
            ClientMessage::SeedOutput(seed) => {
                let image = decode_seed_image(&seed)?;
                self.set_seed_output(image);
            }
            ClientMessage::Stop => self.request_stop(),
        }
        Ok(())
    }

    /// Encodes one produced output frame per the session's current
    /// `output_encoding` and returns ready-to-push wire messages (0 when
    /// the H.264 encoder produced nothing for this input — the normal
    /// startup/buffering case; 1 in the synchronous-per-frame steady
    /// state). Called only from `run_live` (the worker thread).
    fn encode_output(&self, image: &RgbImage, frame_index: u32) -> Vec<Vec<u8>> {
        let output_encoding = self.state.lock().unwrap().output_encoding;
        match output_encoding {
            OutputEncoding::None => Vec::new(),
            OutputEncoding::Raw | OutputEncoding::Png => {
                vec![encode_output_frame(image, output_encoding, frame_index)]
            }
            OutputEncoding::H264 => self.encode_output_h264(image, frame_index),
        }
    }

    #[cfg(feature = "video")]
    fn encode_output_h264(&self, image: &RgbImage, frame_index: u32) -> Vec<Vec<u8>> {
        let mut encoder_slot = self.output_encoder.lock().unwrap();
        let needs_new = match encoder_slot.as_ref() {
            None => true,
            Some(encoder) => encoder.options().width != image.width || encoder.options().height != image.height,
        };
        if needs_new {
            let options = h264::VideoStreamEncoderOptions {
                codec: h264::StreamVideoCodec::H264,
                width: image.width,
                height: image.height,
                fps: 30,
                bitrate_kbps: 4_000,
                keyint: 60,
                low_latency: true,
            };
            match h264::VideoStreamEncoder::new(options) {
                Ok(encoder) => *encoder_slot = Some(encoder),
                Err(e) => {
                    eprintln!("realtime: h264 output encoder init failed, falling back to raw: {e}");
                    return vec![encode_output_frame(image, OutputEncoding::Raw, frame_index)];
                }
            }
        }
        let encoder = encoder_slot.as_mut().unwrap();
        match encoder.push_frame_rgb8(&image.data, frame_index as i64) {
            Ok(packets) => packets
                .into_iter()
                .map(|packet| {
                    let header = FrameHeader {
                        kind: FrameKind::H264,
                        width: image.width.min(u16::MAX as u32) as u16,
                        height: image.height.min(u16::MAX as u32) as u16,
                        frame_index,
                    };
                    realtime_wire::encode_frame(header, &packet.data)
                })
                .collect(),
            Err(e) => {
                eprintln!("realtime: h264 output encode failed, dropping frame: {e}");
                Vec::new()
            }
        }
    }

    #[cfg(not(feature = "video"))]
    fn encode_output_h264(&self, image: &RgbImage, frame_index: u32) -> Vec<Vec<u8>> {
        // LiveParams::from_request refuses output_encoding="h264" without
        // the 'video' feature, and apply_control's OutputEncoding::
        // is_supported_in_this_build filter refuses it there too — this
        // should be unreachable, but degrade to raw rather than silently
        // dropping every frame if it somehow is reached.
        vec![encode_output_frame(image, OutputEncoding::Raw, frame_index)]
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
    merge_feedback_fields(
        config,
        update.feedback,
        update.anchor_follow,
        update.noise_mode.as_deref(),
        update.camera.as_ref(),
        update.drift.as_ref(),
    );
}

/// Decodes a raw or PNG input frame. `handle_binary` routes `H264` frames
/// to `handle_binary_h264` instead (a streaming decoder needs to persist
/// across calls; this function is stateless).
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
        FrameKind::H264 => Err(AssetAiError::Backend(
            "decode_frame_payload: H264 must go through handle_binary_h264 (stateful decoder)".to_string(),
        )),
    }
}

/// Encodes one output frame as raw RGB8 or PNG. NOT used for `H264` — see
/// `RealtimeSession::encode_output`, which dispatches to this for `Raw`/
/// `Png` and to the per-session `VideoStreamEncoder` otherwise; a
/// (should-be-unreachable) `H264` call here falls back to raw defensively
/// rather than panicking the worker thread.
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
        OutputEncoding::H264 | OutputEncoding::None => {
            eprintln!("realtime: encode_output_frame called with non-frame encoding (should route through encode_output) - using raw");
            (FrameKind::Raw, image.data.clone())
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

/// The feedback-loop camera warp with clamp-to-edge borders — see
/// [`warp_feedback_bordered`], which this is the `BorderMode::Clamp` case
/// of. Kept as the plain entry point `testpattern` and the tests use.
pub fn warp_feedback(image: &RgbImage, camera: &CameraMotion) -> RgbImage {
    warp_feedback_bordered(image, camera, BorderMode::Clamp, None)
}

/// The feedback-loop camera warp: a plain CPU center-zoom + pan + roll
/// bilinear resample, applied to the session's own previous output before
/// it becomes the next `live_step` init image in `loop_mode = "feedback"`.
/// This is the ONLY place camera motion is applied — backends never warp
/// `LiveFrameIn::init` themselves (see [`CameraMotion`]'s doc). Pixels the
/// motion pulls from outside the frame are filled per `border`
/// (`BorderMode::Source` needs `source`, and falls back to clamping without
/// one). The CUDA depth-parallax version of this warp is a documented
/// follow-up; this bilinear resample is the CPU stand-in.
pub fn warp_feedback_bordered(
    image: &RgbImage,
    camera: &CameraMotion,
    border: BorderMode,
    source: Option<&RgbImage>,
) -> RgbImage {
    let width = image.width;
    let height = image.height;
    if width == 0 || height == 0 || image.data.len() != width as usize * height as usize * 3 {
        return image.clone();
    }
    let source = source.filter(|s| s.width == width && s.height == height && s.data.len() == image.data.len());
    let zoom = (1.0 + camera.dolly * 0.05).max(1.0e-3);
    let (sin_r, cos_r) = camera.roll.sin_cos();
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let max_x = (width - 1) as f32;
    let max_y = (height - 1) as f32;
    let mut out = vec![0u8; image.data.len()];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let rx = dx * cos_r + dy * sin_r;
            let ry = -dx * sin_r + dy * cos_r;
            let sx = cx + rx / zoom - camera.pan_x * width as f32;
            let sy = cy + ry / zoom - camera.pan_y * height as f32;
            let outside = sx < 0.0 || sy < 0.0 || sx > max_x || sy > max_y;
            let rgb = match (border, source) {
                (BorderMode::Reflect, _) if outside => {
                    sample_bilinear_clamped(image, reflect_coord(sx, max_x), reflect_coord(sy, max_y))
                }
                (BorderMode::Source, Some(source)) if outside => sample_bilinear_clamped(source, x as f32, y as f32),
                _ => sample_bilinear_clamped(image, sx, sy),
            };
            let idx = (y as usize * width as usize + x as usize) * 3;
            out[idx..idx + 3].copy_from_slice(&rgb);
        }
    }
    RgbImage { width, height, data: out }
}

/// Mirrors `v` back into `0..=max` (period `2*max`), so a sample that runs
/// off one edge reads the picture folded back on itself.
fn reflect_coord(v: f32, max: f32) -> f32 {
    if max <= 0.0 {
        return 0.0;
    }
    let period = 2.0 * max;
    let mut t = (v % period).abs();
    if t > max {
        t = period - t;
    }
    t
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

/// Bilinear resample to `width x height` (the feedback source is sized to
/// the loop's frame size once, when it arrives).
pub fn resize_bilinear(image: &RgbImage, width: u32, height: u32) -> RgbImage {
    if width == 0 || height == 0 || image.width == 0 || image.height == 0 {
        return RgbImage::blank(width, height);
    }
    if image.width == width && image.height == height {
        return image.clone();
    }
    let sx_scale = image.width as f32 / width as f32;
    let sy_scale = image.height as f32 / height as f32;
    let mut out = vec![0u8; width as usize * height as usize * 3];
    for y in 0..height {
        let sy = (y as f32 + 0.5) * sy_scale - 0.5;
        for x in 0..width {
            let sx = (x as f32 + 0.5) * sx_scale - 0.5;
            let rgb = sample_bilinear_clamped(image, sx, sy);
            let idx = (y as usize * width as usize + x as usize) * 3;
            out[idx..idx + 3].copy_from_slice(&rgb);
        }
    }
    RgbImage { width, height, data: out }
}

/// Per-channel mean and standard deviation of an RGB8 image, in 0..255 —
/// the statistics the drift anchor pulls a fed-back frame toward.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChannelStats {
    pub mean: [f32; 3],
    pub std: [f32; 3],
}

pub fn channel_stats(image: &RgbImage) -> ChannelStats {
    channel_stats_f32(&image.data.iter().map(|v| *v as f32).collect::<Vec<f32>>())
}

fn channel_stats_f32(data: &[f32]) -> ChannelStats {
    let pixels = data.len() / 3;
    if pixels == 0 {
        return ChannelStats::default();
    }
    let mut sum = [0.0f64; 3];
    let mut sum_sq = [0.0f64; 3];
    for pixel in data.chunks_exact(3) {
        for c in 0..3 {
            let v = pixel[c] as f64;
            sum[c] += v;
            sum_sq[c] += v * v;
        }
    }
    let n = pixels as f64;
    let mut stats = ChannelStats::default();
    for c in 0..3 {
        let mean = sum[c] / n;
        let var = (sum_sq[c] / n - mean * mean).max(0.0);
        stats.mean[c] = mean as f32;
        stats.std[c] = var.sqrt() as f32;
    }
    stats
}

/// Rotation of the RGB cube about its grey axis by `degrees` — greys map to
/// themselves (every row sums to 1), hues walk around the wheel.
fn hue_rotation_matrix(degrees: f32) -> [[f32; 3]; 3] {
    let (sin_a, cos_a) = degrees.to_radians().sin_cos();
    let k = (1.0 - cos_a) / 3.0;
    let s = sin_a / 3.0f32.sqrt();
    [
        [cos_a + k, k - s, k + s],
        [k + s, cos_a + k, k - s],
        [k - s, k + s, cos_a + k],
    ]
}

/// The per-iteration colour treatment of a fed-back frame (see
/// [`DriftParams`] for what each term does): hue rotation, gain about
/// mid-grey, the mean/std anchor toward `source`, grain (deterministic per
/// `frame_index`), then a 3x3 unsharp. With every term at its identity
/// value (hue 0, gain 1, anchor 0, grain 0, sharpen 0) this returns the
/// input unchanged.
pub fn colour_drift(image: &RgbImage, drift: &DriftParams, source: &ChannelStats, frame_index: u64) -> RgbImage {
    let width = image.width as usize;
    let height = image.height as usize;
    if width == 0 || height == 0 || image.data.len() != width * height * 3 {
        return image.clone();
    }
    let matrix = hue_rotation_matrix(drift.hue_deg);
    let gain = drift.gain;
    let mut work: Vec<f32> = Vec::with_capacity(image.data.len());
    for pixel in image.data.chunks_exact(3) {
        let r = pixel[0] as f32;
        let g = pixel[1] as f32;
        let b = pixel[2] as f32;
        for row in &matrix {
            let rotated = row[0] * r + row[1] * g + row[2] * b;
            work.push(127.5 + (rotated - 127.5) * gain);
        }
    }

    let anchor = drift.anchor.clamp(0.0, 1.0);
    if anchor > 0.0 {
        let current = channel_stats_f32(&work);
        let mut scale = [1.0f32; 3];
        let mut offset = [0.0f32; 3];
        for c in 0..3 {
            let target_mean = current.mean[c] + (source.mean[c] - current.mean[c]) * anchor;
            let target_std = current.std[c] + (source.std[c] - current.std[c]) * anchor;
            scale[c] = if current.std[c] > 1.0e-3 { target_std / current.std[c] } else { 1.0 };
            offset[c] = target_mean - current.mean[c] * scale[c];
        }
        for pixel in work.chunks_exact_mut(3) {
            for c in 0..3 {
                pixel[c] = pixel[c] * scale[c] + offset[c];
            }
        }
    }

    let grain = drift.grain.clamp(0.0, 1.0) * 255.0;
    if grain > 0.0 {
        let mut state = splitmix64(frame_index ^ 0x5DEE_CE66_D1CE_4E5D) | 1;
        for value in work.iter_mut() {
            // xorshift64: a cheap, deterministic per-frame stream.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = (state >> 40) as f32 / (1u64 << 24) as f32;
            *value += (unit - 0.5) * 2.0 * grain;
        }
    }

    let sharpen = drift.sharpen.max(0.0);
    let mut out = vec![0u8; image.data.len()];
    if sharpen > 0.0 && width >= 2 && height >= 2 {
        for y in 0..height {
            let y0 = y.saturating_sub(1);
            let y1 = (y + 1).min(height - 1);
            for x in 0..width {
                let x0 = x.saturating_sub(1);
                let x1 = (x + 1).min(width - 1);
                for c in 0..3 {
                    let mut sum = 0.0f32;
                    for yy in [y0, y, y1] {
                        for xx in [x0, x, x1] {
                            sum += work[(yy * width + xx) * 3 + c];
                        }
                    }
                    let blur = sum / 9.0;
                    let v = work[(y * width + x) * 3 + c];
                    out[(y * width + x) * 3 + c] = (v + sharpen * (v - blur)).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    } else {
        for (dst, src) in out.iter_mut().zip(work.iter()) {
            *dst = src.round().clamp(0.0, 255.0) as u8;
        }
    }
    RgbImage { width: image.width, height: image.height, data: out }
}

/// `lerp(a, b, t)` per pixel: `t = 0` is `a` (the source), `t = 1` is `b`
/// (the drifted previous output). `b` sets the frame size; `a` is resampled
/// to it if needed.
pub fn lerp_images(a: &RgbImage, b: &RgbImage, t: f32) -> RgbImage {
    let t = t.clamp(0.0, 1.0);
    if b.width == 0 || b.height == 0 || b.data.len() != b.width as usize * b.height as usize * 3 {
        return b.clone();
    }
    let resized;
    let a = if a.width == b.width && a.height == b.height && a.data.len() == b.data.len() {
        a
    } else {
        resized = resize_bilinear(a, b.width, b.height);
        &resized
    };
    let data = a
        .data
        .iter()
        .zip(b.data.iter())
        .map(|(pa, pb)| {
            let v = *pa as f32 + (*pb as f32 - *pa as f32) * t;
            v.round().clamp(0.0, 255.0) as u8
        })
        .collect();
    RgbImage { width: b.width, height: b.height, data }
}

/// Mean absolute per-byte difference (0..255) between two same-sized
/// frames; `None` when they differ in size.
pub fn mean_abs_diff(a: &RgbImage, b: &RgbImage) -> Option<f64> {
    if a.width != b.width || a.height != b.height || a.data.len() != b.data.len() || a.data.is_empty() {
        return None;
    }
    let sum: u64 = a
        .data
        .iter()
        .zip(b.data.iter())
        .map(|(pa, pb)| (*pa as i32 - *pb as i32).unsigned_abs() as u64)
        .sum();
    Some(sum as f64 / a.data.len() as f64)
}

/// The source image a feedback loop anchors to, sized and measured once
/// The accumulated motion of the WORLD: where the camera has carried the
/// source itself since this picture arrived. A per-frame warp of the
/// feedback echo alone cannot make a settled loop move — the anchor is the
/// (followed) source, and the model re-pins the geometry to it every frame,
/// which is why a constantly-zooming camera measurably never zoomed the
/// output. Accumulate the camera into ONE absolute transform and apply it
/// to the ORIGINAL source each frame instead: the anchor itself travels,
/// stays crisp at any age (one resample, not hundreds), and the loop can
/// not settle while any camera term is non-zero — with no runaway, because
/// the anchor is still a clean engraving, just a moving one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceDrift {
    pub zoom: f32,
    pub roll: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub hue: f32,
}

impl Default for SourceDrift {
    fn default() -> Self {
        SourceDrift { zoom: 1.0, roll: 0.0, pan_x: 0.0, pan_y: 0.0, hue: 0.0 }
    }
}

impl SourceDrift {
    pub fn advance(&mut self, camera: &CameraMotion, hue_deg: f32) {
        self.zoom = (self.zoom * (1.0 + camera.dolly * 0.05)).clamp(1.0e-3, 1.0e4);
        self.roll += camera.roll;
        self.pan_x += camera.pan_x;
        self.pan_y += camera.pan_y;
        self.hue = (self.hue + hue_deg).rem_euclid(360.0);
    }

    pub fn is_identity(&self) -> bool {
        self.zoom == 1.0 && self.roll == 0.0 && self.pan_x == 0.0 && self.pan_y == 0.0 && self.hue == 0.0
    }
}

/// The source under its accumulated drift: one absolute affine (the same
/// mapping `warp_feedback_bordered` applies per frame, at the total zoom /
/// roll / pan) and one absolute hue rotation. Border reflect, so the world
/// folds instead of tearing.
pub fn warp_source_drift(image: &RgbImage, drift: &SourceDrift) -> RgbImage {
    let width = image.width;
    let height = image.height;
    if width == 0 || height == 0 || image.data.len() != width as usize * height as usize * 3 {
        return image.clone();
    }
    let mut out = image.clone();
    if !(drift.zoom == 1.0 && drift.roll == 0.0 && drift.pan_x == 0.0 && drift.pan_y == 0.0) {
        let zoom = drift.zoom.max(1.0e-3);
        let (sin_r, cos_r) = drift.roll.sin_cos();
        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;
        let max_x = (width - 1) as f32;
        let max_y = (height - 1) as f32;
        let mut data = vec![0u8; image.data.len()];
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let rx = dx * cos_r + dy * sin_r;
                let ry = -dx * sin_r + dy * cos_r;
                let sx = cx + rx / zoom - drift.pan_x * width as f32;
                let sy = cy + ry / zoom - drift.pan_y * height as f32;
                let rgb = sample_bilinear_clamped(image, reflect_coord(sx, max_x), reflect_coord(sy, max_y));
                let idx = (y as usize * width as usize + x as usize) * 3;
                data[idx..idx + 3].copy_from_slice(&rgb);
            }
        }
        out = RgbImage { width, height, data };
    }
    if drift.hue != 0.0 {
        let matrix = hue_rotation_matrix(drift.hue);
        for pixel in out.data.chunks_exact_mut(3) {
            let (r, g, b) = (pixel[0] as f32, pixel[1] as f32, pixel[2] as f32);
            for (c, row) in matrix.iter().enumerate() {
                pixel[c] = (row[0] * r + row[1] * g + row[2] * b).clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// per (source, frame size).
struct PreparedSource {
    image: RgbImage,
    stats: ChannelStats,
}

/// Per-session feedback state, owned by the worker thread inside
/// [`run_live`]: the source (reference slot 0, or the latest pushed input
/// frame — whichever arrived last) and its prepared copy at the loop's
/// frame size. The previous output lives in `run_live` itself because feed
/// mode keeps it too (for `stats.frame_diff`).
#[derive(Default)]
struct FeedbackState {
    source: Option<RgbImage>,
    prepared: Option<PreparedSource>,
    reference0_seen: u64,
    /// Where the camera has carried this picture so far; a new picture
    /// starts the world still again.
    drift: SourceDrift,
}

impl FeedbackState {
    fn set_source(&mut self, image: RgbImage) {
        if image.width == 0 || image.height == 0 || image.data.len() != image.width as usize * image.height as usize * 3 {
            return;
        }
        self.source = Some(image);
        self.prepared = None;
        self.drift = SourceDrift::default();
    }

    fn prepared(&mut self, width: u32, height: u32) -> Option<&PreparedSource> {
        let source = self.source.as_ref()?;
        let stale = self
            .prepared
            .as_ref()
            .map_or(true, |prepared| prepared.image.width != width || prepared.image.height != height);
        if stale {
            let image = resize_bilinear(source, width, height);
            let stats = channel_stats(&image);
            self.prepared = Some(PreparedSource { image, stats });
        }
        self.prepared.as_ref()
    }
}

/// The picture inside a `seed_output` message: raw RGB8 with its size, or a
/// PNG. One image, nothing else — everything else a session needs it can
/// rebuild for itself.
fn decode_seed_image(seed: &crate::realtime_wire::SeedOutputMessageJson) -> Result<RgbImage, AssetAiError> {
    if let Some(raw_b64) = seed.raw_b64.as_deref() {
        let data = makepad_base64::base64_decode(raw_b64.as_bytes())
            .map_err(|e| AssetAiError::Params(format!("seed_output: bad base64: {e:?}")))?;
        let (width, height) = match (seed.w, seed.h) {
            (Some(w), Some(h)) => (w, h),
            _ => return Err(AssetAiError::Params("seed_output: raw_b64 needs w and h".to_string())),
        };
        let want = width as usize * height as usize * 3;
        if data.len() != want {
            return Err(AssetAiError::Params(format!(
                "seed_output: {} bytes for a {width}x{height} rgb8 image (expected {want})",
                data.len()
            )));
        }
        return Ok(RgbImage { width, height, data });
    }
    let png_b64 = seed
        .png_b64
        .as_deref()
        .ok_or_else(|| AssetAiError::Params("seed_output: needs raw_b64 (with w/h) or png_b64".to_string()))?;
    let bytes = makepad_base64::base64_decode(png_b64.as_bytes())
        .map_err(|e| AssetAiError::Params(format!("seed_output: bad base64: {e:?}")))?;
    let (data, width, height) = crate::testpattern::decode_png_rgb8(&bytes)?;
    Ok(RgbImage { width, height, data })
}

/// One feedback iteration's images: the sampler init (None on a cold
/// start) and the anchor the edit conditions on. Pure — see the tests.
pub fn feedback_frame(
    source: &RgbImage,
    source_stats: &ChannelStats,
    previous: Option<&RgbImage>,
    config: &LiveConfig,
    frame_index: u64,
) -> Option<RgbImage> {
    let previous = previous?;
    let warped = warp_feedback_bordered(previous, &config.camera, config.drift.border, Some(source));
    let drifted = colour_drift(&warped, &config.drift, source_stats, frame_index);
    Some(lerp_images(source, &drifted, config.feedback))
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
    let started = Instant::now();
    {
        let codec = session.codec_stats();
        eprintln!(
            "realtime {}: live session open — model {}, {} in, {} out",
            session.job_id, session.model_id, codec.input, codec.output
        );
    }
    let mut last_output: Option<RgbImage> = None;
    let mut feedback = FeedbackState::default();
    let mut frame_index: u64 = 0;
    let mut last_progress_push = Instant::now();
    let mut last_log = Instant::now();
    let mut idle_since: Option<Instant> = None;

    // The outbound encode never sits between two model steps: the loop hands
    // each finished frame to this thread (latest wins) and immediately goes
    // round again; only the copy streamed to the client is ever encoded.
    let outbound: Mutex<Option<(u64, RgbImage)>> = Mutex::new(None);
    let outbound_cv = Condvar::new();
    let outbound_done = AtomicBool::new(false);
    let result = std::thread::scope(|threads| {
    threads.spawn(|| {
        loop {
            let item = {
                let mut slot = outbound.lock().unwrap();
                loop {
                    if let Some(item) = slot.take() {
                        break item;
                    }
                    if outbound_done.load(Ordering::Relaxed) {
                        return;
                    }
                    slot = outbound_cv.wait(slot).unwrap();
                }
            };
            let encode_start = Instant::now();
            for frame_bytes in session.encode_output(&item.1, item.0 as u32) {
                session.push_bytes(frame_bytes);
            }
            session
                .last_encode_ms
                .store(encode_start.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
    });
    let mut worker = || -> Result<(), AssetAiError> {
    'session: loop {
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
        let (mut config, loop_mode, _output_encoding, max_fps) = session.snapshot();

        let prep_start = Instant::now();
        if session.take_reset() {
            last_output = None;
        }
        // A feed that moved here from another box: its last frame becomes
        // this session's previous output, so the next iteration continues
        // the trip rather than cold-starting one full edit from the source.
        // The size is normalised with the rest below.
        if let Some(seed) = session.take_seed_output() {
            last_output = Some(seed);
        }
        let (init_image, anchor_image): (Option<RgbImage>, Option<RgbImage>) = match loop_mode {
            LoopMode::Feed => {
                let frame = loop {
                    cancel.check()?;
                    if session.stop_requested() {
                        return Ok(());
                    }
                    if let Some(frame) = session.take_mailbox_frame() {
                        break frame;
                    }
                    if session.socket_count() == 0 {
                        // Nobody listening and nobody feeding: a client that
                        // died without `stop` would otherwise park this
                        // branch forever and hold the GPU slot. Go back
                        // through the top, where the idle timeout counts a
                        // socketless session down and ends it.
                        session.wait_for_mailbox(Duration::from_millis(250));
                        continue 'session;
                    }
                    if session.loop_mode() != LoopMode::Feed {
                        // A control update flipped the session to feedback
                        // while this branch sat waiting for input. Re-enter
                        // the loop under the new mode instead of waiting for
                        // a frame the client will never push.
                        continue 'session;
                    }
                    session.wait_for_mailbox(Duration::from_millis(250));
                };
                // Remembered as the feedback source, so a client that opens
                // in feed mode and flips to feedback keeps the picture it
                // last pushed as its anchor. Invisible to the backend.
                feedback.set_source(frame.clone());
                (Some(frame), None)
            }
            LoopMode::Feedback => {
                if session.socket_count() == 0 {
                    // A free-running loop with nobody listening would paint
                    // frames straight into the void (push_bytes drops them)
                    // and hold the GPU while doing it. Hold the loop instead
                    // — sources keep landing in the mailbox/reference slots
                    // — and let the idle timeout above end the session.
                    session.wait_for_mailbox(Duration::from_millis(250));
                    continue 'session;
                }
                // Whichever arrived last wins: a pushed frame or a new
                // reference slot 0 is a retarget.
                if let Some(frame) = session.take_mailbox_frame() {
                    feedback.set_source(frame);
                }
                if let Some(reference) = session.take_new_reference0(&mut feedback.reference0_seen) {
                    feedback.set_source(reference);
                }
                // The loop runs at the (backend-rounded) frame size. A size
                // change mid-loop — a control width/height — carries the
                // previous output over to the new size instead of stopping
                // the session on an init/output mismatch.
                let width = config.width.max(16) / 16 * 16;
                let height = config.height.max(16) / 16 * 16;
                if let Some(previous) = last_output.as_ref() {
                    if previous.width != width || previous.height != height {
                        last_output = Some(resize_bilinear(previous, width, height));
                    }
                }
                // Slot 0 IS the source in feedback mode — the anchor carries
                // it, so it must not ride along a second time as an extra
                // reference.
                if !config.references.is_empty() {
                    config.references.remove(0);
                }
                // The camera moves the WORLD: the source itself drifts under
                // the accumulated transform, so the anchor travels with it
                // and the loop cannot settle while any camera term is on —
                // see SourceDrift for why warping the echo alone cannot.
                let drift_now = feedback.drift;
                feedback.drift.advance(&config.camera, config.drift.hue_deg);
                let Some(prepared) = feedback.prepared(width, height) else {
                    session.wait_for_mailbox(Duration::from_millis(250));
                    continue;
                };
                let (moved_source, moved_stats) = if drift_now.is_identity() {
                    (prepared.image.clone(), prepared.stats)
                } else {
                    let moved = warp_source_drift(&prepared.image, &drift_now);
                    let stats = channel_stats(&moved);
                    (moved, stats)
                };
                let init = feedback_frame(&moved_source, &moved_stats, last_output.as_ref(), &config, frame_index);
                if init.is_none() {
                    // Cold start: one full edit of the source, from noise.
                    config.strength = 1.0;
                }
                // The anchor the edit conditions on. Pinned to the still
                // source it pins the trip with it — the loop converges to a
                // still whatever the prompt or the carry. It rides the
                // moving source, and `anchor_follow` lets it also lean into
                // the previous output.
                let anchor = match last_output.as_ref().filter(|_| config.anchor_follow > 0.0) {
                    Some(last) if last.width == moved_source.width && last.height == moved_source.height => {
                        lerp_images(&moved_source, last, config.anchor_follow)
                    }
                    _ => moved_source.clone(),
                };
                (init, Some(anchor))
            }
        };
        match config.noise_mode.resolve(loop_mode) {
            NoiseMode::Hold => {}
            NoiseMode::Reroll | NoiseMode::Auto => resolve_seed(&mut config, frame_index),
        }
        let prep_ms = prep_start.elapsed().as_secs_f64() * 1000.0;

        cancel.check()?;
        let step = backend.live_step(
            LiveFrameIn {
                init: init_image.as_ref(),
                anchor: anchor_image.as_ref(),
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

        if let Some(aux_json) = out.aux_json.as_deref() {
            session.push_bytes(realtime_wire::encode_aux_message(frame_index, aux_json));
        }
        {
            let mut slot = outbound.lock().unwrap();
            if slot.replace((frame_index, out.image.clone())).is_some() {
                session.dropped_encode.fetch_add(1, Ordering::Relaxed);
            }
            outbound_cv.notify_one();
        }
        // What `post` reports now: the most recent COMPLETED encode, which
        // runs beside the next model step instead of before it.
        let post_ms = session.last_encode_ms.load(Ordering::Relaxed) as f64;
        let frame_diff = last_output.as_ref().and_then(|previous| mean_abs_diff(previous, &out.image));
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
                    text_encode: out.text_encode_ms,
                    post: post_ms,
                },
                frames_in,
                frames_out,
                dropped,
                codec: session.codec_stats(),
                loop_mode: loop_mode.as_str().to_string(),
                frame_diff,
            })
            .into_bytes(),
        );

        if last_progress_push.elapsed() >= Duration::from_millis(100) {
            progress("live", frames_in, frames_out, fps);
            last_progress_push = Instant::now();
        }
        if loop_mode == LoopMode::Feedback && last_log.elapsed() >= Duration::from_secs(5) {
            last_log = Instant::now();
            eprintln!(
                "realtime {}: feedback frame {frame_index} {fps:.2} fps, model {:.0} ms (text encode {:.0} ms), \
                 prep {prep_ms:.0} ms, frame diff {}",
                session.job_id,
                out.model_ms,
                out.text_encode_ms,
                frame_diff.map_or("n/a".to_string(), |d| format!("{d:.2}")),
            );
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
    };
    let result = worker();
    outbound_done.store(true, Ordering::Relaxed);
    outbound_cv.notify_all();
    result
    });
    // One line per session in the service log: enough to tell from a
    // headless box whether the wire decoded and how fast the model ran.
    let elapsed = started.elapsed().as_secs_f64();
    let frames_in = session.frames_in.load(Ordering::Relaxed);
    let frames_out = session.frames_out.load(Ordering::Relaxed);
    let codec = session.codec_stats();
    eprintln!(
        "realtime {}: session closed after {elapsed:.1} s — {} {} frames in, {frames_out} out ({:.1} fps), \
         {} dropped, {} undecodable, {} unencoded{}",
        session.job_id,
        codec.input,
        frames_in,
        if elapsed > 0.0 { frames_out as f64 / elapsed } else { 0.0 },
        session.dropped.load(Ordering::Relaxed),
        codec.dropped_decode,
        session.dropped_encode.load(Ordering::Relaxed),
        match &result {
            Ok(()) => String::new(),
            Err(e) => format!(", error: {e}"),
        }
    );
    result
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
    fn control_merge_feedback_fields_are_partial_and_clamped() {
        let mut config = blank_config();
        let update = ControlUpdateJson {
            kind: "control".to_string(),
            feedback: Some(1.7), // clamps to 1.0
            noise_mode: Some("reroll".to_string()),
            drift: Some(crate::realtime_wire::DriftUpdateJson {
                hue: Some(2.0),
                border: Some("source".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        apply_control_to_config(&mut config, &update);
        assert_eq!(config.feedback, 1.0);
        assert_eq!(config.noise_mode, NoiseMode::Reroll);
        assert_eq!(config.drift.hue_deg, 2.0);
        assert_eq!(config.drift.border, BorderMode::Source);
        // Drift fields the message did not carry keep their defaults.
        assert_eq!(config.drift.gain, 0.98);
        assert_eq!(config.drift.anchor, 0.05);
        assert_eq!(config.drift.grain, 0.02);
        assert_eq!(config.drift.sharpen, 0.1);

        // Unknown enum strings are ignored, never fatal.
        let bad = ControlUpdateJson {
            kind: "control".to_string(),
            noise_mode: Some("nonsense".to_string()),
            drift: Some(crate::realtime_wire::DriftUpdateJson {
                border: Some("nonsense".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        apply_control_to_config(&mut config, &bad);
        assert_eq!(config.noise_mode, NoiseMode::Reroll);
        assert_eq!(config.drift.border, BorderMode::Source);
    }

    /// A pinned anchor pins the whole feed (the loop converges to a still),
    /// so the follow knob must ride the control wire and clamp like the
    /// other unit fields — and stay put when a message does not carry it.
    #[test]
    fn anchor_follow_rides_the_control_wire_and_clamps() {
        let mut config = blank_config();
        assert_eq!(config.anchor_follow, 0.0);
        let update = ControlUpdateJson {
            kind: "control".to_string(),
            anchor_follow: Some(0.6),
            ..Default::default()
        };
        apply_control_to_config(&mut config, &update);
        assert_eq!(config.anchor_follow, 0.6);
        let over = ControlUpdateJson {
            kind: "control".to_string(),
            anchor_follow: Some(1.7), // clamps to 1.0
            ..Default::default()
        };
        apply_control_to_config(&mut config, &over);
        assert_eq!(config.anchor_follow, 1.0);
        let silent = ControlUpdateJson { kind: "control".to_string(), ..Default::default() };
        apply_control_to_config(&mut config, &silent);
        assert_eq!(config.anchor_follow, 1.0);
    }

    #[test]
    fn open_request_parses_feedback_fields_and_refuses_bad_enums() {
        use crate::protocol::RealtimeRequestJson;
        let request = RealtimeRequestJson {
            model: "testpattern".to_string(),
            loop_mode: Some("feedback".to_string()),
            feedback: Some(0.3),
            noise_mode: Some("hold".to_string()),
            camera: Some(CameraUpdateJson { dolly: Some(0.2), roll: Some(0.01), ..Default::default() }),
            drift: Some(crate::realtime_wire::DriftUpdateJson { gain: Some(0.9), ..Default::default() }),
            ..Default::default()
        };
        let params = LiveParams::from_request(&request).unwrap();
        assert_eq!(params.loop_mode, LoopMode::Feedback);
        assert_eq!(params.config.feedback, 0.3);
        assert_eq!(params.config.noise_mode, NoiseMode::Hold);
        assert_eq!(params.config.camera.dolly, 0.2);
        assert_eq!(params.config.camera.roll, 0.01);
        assert_eq!(params.config.drift.gain, 0.9);
        // The defaults are STILL now — the camera moves the source itself
        // since SourceDrift, so any default motion would run away with every
        // plain session. Motion is asked for, never assumed.
        assert_eq!(params.config.drift.hue_deg, 0.0);

        // An explicit zero stays zero, and an explicit value lands.
        let still = RealtimeRequestJson {
            model: "testpattern".to_string(),
            camera: Some(CameraUpdateJson { dolly: Some(0.0), roll: Some(0.0), ..Default::default() }),
            ..Default::default()
        };
        let params = LiveParams::from_request(&still).unwrap();
        assert_eq!(params.config.camera, CameraMotion::default());

        let bad = RealtimeRequestJson {
            model: "testpattern".to_string(),
            noise_mode: Some("sometimes".to_string()),
            ..Default::default()
        };
        assert!(LiveParams::from_request(&bad).is_err());

        // Defaults: a plain request is feedback 0.7 / noise auto / the slow
        // tunnel + spiral camera.
        let plain = RealtimeRequestJson { model: "testpattern".to_string(), ..Default::default() };
        let params = LiveParams::from_request(&plain).unwrap();
        assert_eq!(params.config.feedback, 0.7);
        assert_eq!(params.config.noise_mode, NoiseMode::Auto);
        assert_eq!(params.config.camera, CameraMotion::feedback_default());
        assert_eq!(params.config.noise_mode.resolve(LoopMode::Feedback), NoiseMode::Hold);
        assert_eq!(params.config.noise_mode.resolve(LoopMode::Feed), NoiseMode::Reroll);
    }

    #[test]
    fn output_encoding_none_is_feed_only() {
        use crate::protocol::RealtimeRequestJson;

        let feed = RealtimeRequestJson {
            model: "testpattern".to_string(),
            loop_mode: Some("feed".to_string()),
            output_encoding: Some("none".to_string()),
            ..Default::default()
        };
        let params = LiveParams::from_request(&feed).unwrap();
        assert_eq!(params.output_encoding, OutputEncoding::None);

        let feedback = RealtimeRequestJson {
            model: "testpattern".to_string(),
            loop_mode: Some("feedback".to_string()),
            output_encoding: Some("none".to_string()),
            ..Default::default()
        };
        let error = LiveParams::from_request(&feedback)
            .err()
            .expect("feedback with no output must be refused");
        assert!(matches!(error, AssetAiError::Params(_)));
        assert!(error.to_string().contains("feedback"));
        assert!(error.to_string().contains("output_encoding \"none\""));
    }

    #[test]
    fn strength_is_a_five_position_switch_at_four_steps() {
        use crate::backend::img2img_start_step;
        assert_eq!(img2img_start_step(0.0, 4), 4);
        assert_eq!(img2img_start_step(0.2, 4), 3);
        assert_eq!(img2img_start_step(0.45, 4), 2);
        assert_eq!(img2img_start_step(0.5, 4), 2);
        assert_eq!(img2img_start_step(0.75, 4), 1);
        // The cliff: anything above 0.75 starts at step 0, where the init
        // is never encoded at all.
        assert_eq!(img2img_start_step(0.76, 4), 0);
        assert_eq!(img2img_start_step(1.0, 4), 0);
        assert_eq!(img2img_start_step(0.45, 8), 4);
    }

    fn gradient_image(width: u32, height: u32) -> RgbImage {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                data.push((x * 255 / width.max(1)) as u8);
                data.push((y * 255 / height.max(1)) as u8);
                data.push(((x + y) * 255 / (width + height).max(1)) as u8);
            }
        }
        RgbImage { width, height, data }
    }

    fn solid_image(width: u32, height: u32, rgb: [u8; 3]) -> RgbImage {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..width * height {
            data.extend_from_slice(&rgb);
        }
        RgbImage { width, height, data }
    }

    #[test]
    fn lerp_images_blends_source_and_drifted() {
        let source = solid_image(4, 4, [0, 100, 200]);
        let drifted = solid_image(4, 4, [200, 100, 0]);
        assert_eq!(lerp_images(&source, &drifted, 0.0).data, source.data);
        assert_eq!(lerp_images(&source, &drifted, 1.0).data, drifted.data);
        let half = lerp_images(&source, &drifted, 0.5);
        assert_eq!(&half.data[0..3], &[100, 100, 100]);
        // A source of another size is resampled to the drifted frame.
        let small = solid_image(2, 2, [0, 100, 200]);
        let mixed = lerp_images(&small, &drifted, 0.0);
        assert_eq!((mixed.width, mixed.height), (4, 4));
        assert_eq!(&mixed.data[0..3], &[0, 100, 200]);
    }

    fn identity_drift() -> DriftParams {
        DriftParams { hue_deg: 0.0, gain: 1.0, anchor: 0.0, grain: 0.0, sharpen: 0.0, border: BorderMode::Reflect }
    }

    /// The world moves: the accumulated drift compounds, resets with a new
    /// picture, and its absolute warp really displaces the pixels — the
    /// mechanism that makes a camera perturbation land as MOTION instead of
    /// being repainted away by the anchor.
    #[test]
    fn source_drift_compounds_resets_and_moves_the_picture() {
        let camera = CameraMotion { dolly: 0.4, pan_x: 0.01, pan_y: 0.0, roll: 0.02 };
        let mut drift = SourceDrift::default();
        assert!(drift.is_identity());
        drift.advance(&camera, 1.5);
        drift.advance(&camera, 1.5);
        assert!((drift.zoom - 1.02f32 * 1.02).abs() < 1e-5);
        assert!((drift.roll - 0.04).abs() < 1e-6);
        assert!((drift.pan_x - 0.02).abs() < 1e-6);
        assert_eq!(drift.hue, 3.0);
        // A 32x32 field with one bright pixel off-centre: a big zoom must
        // move it, and identity must not touch a byte.
        let mut image = RgbImage { width: 32, height: 32, data: vec![10u8; 32 * 32 * 3] };
        let at = (10usize * 32 + 20) * 3;
        image.data[at] = 250;
        let same = warp_source_drift(&image, &SourceDrift::default());
        assert_eq!(same.data, image.data);
        let zoomed = warp_source_drift(&image, &SourceDrift { zoom: 2.0, ..Default::default() });
        assert_ne!(zoomed.data, image.data);
        assert!(zoomed.data[at] < 200, "the marker should have moved away from its old spot");
        // Hue-only: the same red patch turns, geometry untouched.
        let red = RgbImage { width: 4, height: 4, data: [200u8, 20, 20].repeat(16) };
        let turned = warp_source_drift(&red, &SourceDrift { hue: 120.0, ..Default::default() });
        assert!(turned.data[1] > turned.data[0], "red should have rotated toward green: {:?}", &turned.data[..3]);
        // A fresh picture starts the world still again.
        let mut feedback = FeedbackState::default();
        feedback.drift = drift;
        feedback.set_source(red);
        assert!(feedback.drift.is_identity());
    }

    #[test]
    fn colour_drift_identity_terms_return_the_input() {
        let image = gradient_image(8, 6);
        let stats = channel_stats(&image);
        let out = colour_drift(&image, &identity_drift(), &stats, 3);
        assert_eq!(out.data, image.data);
    }

    #[test]
    fn colour_drift_hue_rotates_red_toward_green_and_keeps_grey() {
        let mut image = solid_image(2, 1, [200, 0, 0]);
        image.data[3..6].copy_from_slice(&[90, 90, 90]);
        let stats = channel_stats(&image);
        let drift = DriftParams { hue_deg: 120.0, ..identity_drift() };
        let out = colour_drift(&image, &drift, &stats, 0);
        // Red -> green after a third of a turn.
        assert!(out.data[1] > 180 && out.data[0] < 20 && out.data[2] < 20, "got {:?}", &out.data[0..3]);
        // Grey lies on the rotation axis and does not move.
        assert_eq!(&out.data[3..6], &[90, 90, 90]);
    }

    #[test]
    fn colour_drift_gain_pulls_toward_mid_grey() {
        let image = solid_image(2, 2, [255, 0, 128]);
        let stats = channel_stats(&image);
        let drift = DriftParams { gain: 0.5, ..identity_drift() };
        let out = colour_drift(&image, &drift, &stats, 0);
        assert_eq!(&out.data[0..3], &[191, 64, 128]);
    }

    #[test]
    fn colour_drift_anchor_pulls_stats_toward_source() {
        // A bright, high-contrast frame against a dark, flat source.
        let frame = gradient_image(16, 16);
        let source = ChannelStats { mean: [40.0, 40.0, 40.0], std: [5.0, 5.0, 5.0] };
        let before = channel_stats(&frame);
        let full = colour_drift(&frame, &DriftParams { anchor: 1.0, ..identity_drift() }, &source, 0);
        let after = channel_stats(&full);
        for c in 0..3 {
            assert!((after.mean[c] - source.mean[c]).abs() < 1.5, "mean[{c}] {} vs {}", after.mean[c], source.mean[c]);
            assert!((after.std[c] - source.std[c]).abs() < 1.5, "std[{c}] {} vs {}", after.std[c], source.std[c]);
        }
        // 5% moves the statistics 5% of the way, not all the way.
        let five = colour_drift(&frame, &DriftParams { anchor: 0.05, ..identity_drift() }, &source, 0);
        let partial = channel_stats(&five);
        let expected = before.mean[0] + (source.mean[0] - before.mean[0]) * 0.05;
        assert!((partial.mean[0] - expected).abs() < 1.5, "{} vs {}", partial.mean[0], expected);
        assert!(partial.mean[0] > after.mean[0] + 10.0);
    }

    #[test]
    fn colour_drift_grain_is_deterministic_per_frame_and_bounded() {
        let image = solid_image(8, 8, [100, 100, 100]);
        let stats = channel_stats(&image);
        let drift = DriftParams { grain: 0.02, ..identity_drift() };
        let a = colour_drift(&image, &drift, &stats, 7);
        let b = colour_drift(&image, &drift, &stats, 7);
        let c = colour_drift(&image, &drift, &stats, 8);
        assert_eq!(a.data, b.data);
        assert_ne!(a.data, c.data);
        assert!(a.data.iter().all(|v| (*v as i32 - 100).abs() <= 6));
        assert!(a.data.iter().any(|v| *v != 100));
    }

    #[test]
    fn colour_drift_sharpen_raises_local_contrast() {
        let mut image = solid_image(5, 5, [100, 100, 100]);
        let center = (2 * 5 + 2) * 3;
        image.data[center..center + 3].copy_from_slice(&[140, 140, 140]);
        let stats = channel_stats(&image);
        let drift = DriftParams { sharpen: 1.0, ..identity_drift() };
        let out = colour_drift(&image, &drift, &stats, 0);
        assert!(out.data[center] > 140, "center {}", out.data[center]);
        let neighbour = (2 * 5 + 1) * 3;
        assert!(out.data[neighbour] < 100, "neighbour {}", out.data[neighbour]);
    }

    #[test]
    fn warp_border_modes_differ_only_outside_the_frame() {
        let image = gradient_image(16, 16);
        let source = solid_image(16, 16, [7, 7, 7]);
        // Dolly OUT (zoom 0.8): the outer band samples outside the frame.
        let camera = CameraMotion { dolly: -4.0, ..Default::default() };
        let clamp = warp_feedback_bordered(&image, &camera, BorderMode::Clamp, Some(&source));
        let reflect = warp_feedback_bordered(&image, &camera, BorderMode::Reflect, Some(&source));
        let from_source = warp_feedback_bordered(&image, &camera, BorderMode::Source, Some(&source));
        assert_ne!(clamp.data, reflect.data);
        // The corner pixel under `source` fill is the source's pixel.
        assert_eq!(&from_source.data[0..3], &[7, 7, 7]);
        assert_ne!(&clamp.data[0..3], &[7, 7, 7]);
        // The center is inside the frame for every mode and identical.
        let center = (8 * 16 + 8) * 3;
        assert_eq!(&clamp.data[center..center + 3], &reflect.data[center..center + 3]);
        assert_eq!(&clamp.data[center..center + 3], &from_source.data[center..center + 3]);
        // Reflect folds the edge back: the corner reads an interior value,
        // not the smeared edge value clamp produces.
        assert_ne!(&reflect.data[0..3], &clamp.data[0..3]);
        // Source fill without a source degrades to clamp.
        let no_source = warp_feedback_bordered(&image, &camera, BorderMode::Source, None);
        assert_eq!(no_source.data, clamp.data);
    }

    #[test]
    fn warp_zoom_and_roll_move_a_marker_as_expected() {
        let mut image = solid_image(8, 8, [0, 0, 0]);
        // Marker right of center at (7, 4).
        let marker = (4 * 8 + 7) * 3;
        image.data[marker..marker + 3].copy_from_slice(&[255, 255, 255]);
        // A quarter-turn roll: output (4, 7) samples the input at (7, 4).
        let rolled = warp_feedback(&image, &CameraMotion { roll: std::f32::consts::FRAC_PI_2, ..Default::default() });
        let below = (7 * 8 + 4) * 3;
        assert_eq!(&rolled.data[below..below + 3], &[255, 255, 255]);
        assert_eq!(&rolled.data[marker..marker + 3], &[0, 0, 0]);
        // Dolly 0.2 = zoom 1.01: the default feedback zoom is a 1% creep,
        // which on an 8px frame moves nothing a full pixel — the marker
        // survives in place (bilinear-attenuated at most).
        let zoomed = warp_feedback(&image, &CameraMotion { dolly: 0.2, ..Default::default() });
        assert!(zoomed.data[marker] > 200);
        // Dolly 20 = zoom 2x: output (7,4) now samples (5.5,4) — black —
        // and the marker itself would land at output x = 10, off-frame, so
        // the whole frame goes black.
        let zoomed2 = warp_feedback(&image, &CameraMotion { dolly: 20.0, ..Default::default() });
        assert_eq!(zoomed2.data[marker], 0);
        assert!(zoomed2.data.iter().all(|v| *v == 0));
    }

    #[test]
    fn feedback_frame_cold_start_has_no_init_and_blend_follows_feedback() {
        let source = gradient_image(16, 16);
        let stats = channel_stats(&source);
        let mut config = blank_config();
        config.drift = identity_drift();
        config.camera = CameraMotion::default();
        assert!(feedback_frame(&source, &stats, None, &config, 0).is_none());

        let previous = solid_image(16, 16, [200, 50, 50]);
        config.feedback = 0.0;
        let init = feedback_frame(&source, &stats, Some(&previous), &config, 1).unwrap();
        assert_eq!(init.data, source.data, "feedback 0 = the source itself");
        config.feedback = 1.0;
        let init = feedback_frame(&source, &stats, Some(&previous), &config, 1).unwrap();
        assert_eq!(init.data, previous.data, "feedback 1 with identity drift/camera = the previous output");
        config.feedback = 0.7;
        let init = feedback_frame(&source, &stats, Some(&previous), &config, 1).unwrap();
        assert_ne!(init.data, source.data);
        assert_ne!(init.data, previous.data);
    }

    #[test]
    fn mean_abs_diff_measures_motion() {
        let a = solid_image(4, 4, [10, 10, 10]);
        let b = solid_image(4, 4, [13, 10, 7]);
        assert_eq!(mean_abs_diff(&a, &a), Some(0.0));
        assert_eq!(mean_abs_diff(&a, &b), Some(2.0));
        assert_eq!(mean_abs_diff(&a, &solid_image(2, 2, [0, 0, 0])), None);
    }

    /// A backend that records exactly what `run_live` hands it.
    #[derive(Clone, Debug)]
    struct SeenFrame {
        frame_index: u64,
        has_init: bool,
        has_anchor: bool,
        anchor_matches_source: bool,
        init_size: Option<(u32, u32)>,
        anchor_size: Option<(u32, u32)>,
        strength: f32,
        seed: u64,
        references: usize,
    }

    struct RecordingBackend {
        seen: std::sync::Arc<Mutex<Vec<SeenFrame>>>,
        source: RgbImage,
    }

    impl ContentBackend for RecordingBackend {
        fn model_id(&self) -> &str {
            "recording"
        }
        fn ensure_loaded(&mut self, _ctx: &mut crate::backend::BackendCtx) -> Result<(), AssetAiError> {
            Ok(())
        }
        fn generate(
            &mut self,
            _params: &crate::backend::GenerateParams,
            _progress: crate::backend::ProgressSink,
            _cancel: &CancelToken,
        ) -> Result<Vec<crate::backend::ArtifactData>, AssetAiError> {
            Err(AssetAiError::Backend("live only".into()))
        }
        fn live_supported(&self) -> bool {
            true
        }
        fn live_step(&mut self, frame: LiveFrameIn<'_>, _cancel: &CancelToken) -> Result<crate::backend::LiveFrameOut, AssetAiError> {
            self.seen.lock().unwrap().push(SeenFrame {
                frame_index: frame.frame_index,
                has_init: frame.init.is_some(),
                has_anchor: frame.anchor.is_some(),
                anchor_matches_source: frame.anchor.map_or(false, |a| a.data == self.source.data),
                init_size: frame.init.map(|i| (i.width, i.height)),
                anchor_size: frame.anchor.map(|a| (a.width, a.height)),
                strength: frame.config.strength,
                seed: frame.config.seed,
                references: frame.config.references.len(),
            });
            // A frame that changes every step, so frame_diff is non-zero.
            let tint = (frame.frame_index * 40 % 256) as u8;
            let image = solid_image(frame.config.width, frame.config.height, [tint, 20, 30]);
            Ok(crate::backend::LiveFrameOut {
                image,
                aux_json: None,
                model_ms: 0.1,
                text_encode_ms: 0.0,
            })
        }
    }

    fn run_recording_session(
        params: LiveParams,
        setup: impl FnOnce(&RealtimeSession) + Send + 'static,
        done: impl Fn(&[SeenFrame]) -> bool,
        source: RgbImage,
    ) -> Vec<SeenFrame> {
        let session = std::sync::Arc::new(RealtimeSession::new("job-t".to_string(), &params));
        // A feedback worker only runs while somebody listens; give the test
        // session one socket for its whole life.
        let (socket_tx, socket_rx) = mpsc::channel();
        session.add_socket(1, socket_tx);
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let worker = {
            let session = session.clone();
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut backend = RecordingBackend { seen, source };
                let cancel = CancelToken::new();
                run_live(&session, &mut backend, &cancel, |_, _, _, _| {})
            })
        };
        setup(&session);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&seen.lock().unwrap()) {
            assert!(Instant::now() < deadline, "timed out waiting for the session to reach the wanted state");
            std::thread::sleep(Duration::from_millis(5));
        }
        session.request_stop();
        worker.join().unwrap().unwrap();
        drop(socket_rx);
        let frames = seen.lock().unwrap().clone();
        frames
    }

    fn recording_params(loop_mode: LoopMode) -> LiveParams {
        let mut config = LiveConfig::default();
        config.width = 32;
        config.height = 32;
        config.strength = 0.45;
        config.seed = 7;
        config.seed_mode = SeedMode::Increment;
        LiveParams {
            model: "recording".to_string(),
            config,
            loop_mode,
            input_encoding: OutputEncoding::Raw,
            output_encoding: OutputEncoding::Raw,
            max_fps: 200.0,
            idle_timeout_s: 0,
        }
    }

    /// A feed-mode client that vanishes without `stop` (a crash, a kill)
    /// must not hold the box's live slot forever: once its socket is gone
    /// the idle timeout ends the session even though no frame ever arrives
    /// again.
    #[test]
    fn run_live_feed_mode_ends_when_the_last_socket_leaves_past_the_idle_timeout() {
        let mut params = recording_params(LoopMode::Feed);
        params.idle_timeout_s = 1;
        let session = std::sync::Arc::new(RealtimeSession::new("job-t".to_string(), &params));
        let (socket_tx, socket_rx) = mpsc::channel();
        session.add_socket(1, socket_tx);
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let source = gradient_image(32, 32);
        let worker = {
            let session = session.clone();
            let seen = seen.clone();
            let source = source.clone();
            std::thread::spawn(move || {
                let mut backend = RecordingBackend { seen, source };
                let cancel = CancelToken::new();
                run_live(&session, &mut backend, &cancel, |_, _, _, _| {})
            })
        };
        session.push_input_frame(source);
        let deadline = Instant::now() + Duration::from_secs(10);
        while seen.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the first frame never ran");
            std::thread::sleep(Duration::from_millis(5));
        }
        // The client is gone: socket closed, no stop, no more frames.
        session.remove_socket(1);
        drop(socket_rx);
        let started = Instant::now();
        let deadline = started + Duration::from_secs(8);
        while !worker.is_finished() {
            assert!(Instant::now() < deadline, "a socketless feed session must end on the idle timeout");
            std::thread::sleep(Duration::from_millis(20));
        }
        worker.join().unwrap().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(900), "ended before the idle timeout");
    }

    #[test]
    fn run_live_feed_mode_passes_init_without_anchor_and_rerolls_the_seed() {
        let source = gradient_image(32, 32);
        let pushed = source.clone();
        let frames = run_recording_session(
            recording_params(LoopMode::Feed),
            move |session| {
                session.push_input_frame(pushed.clone());
                session.push_input_frame(pushed);
            },
            |frames| frames.len() >= 1,
            source,
        );
        let first = &frames[0];
        assert!(first.has_init);
        assert!(!first.has_anchor, "feed mode never passes an anchor");
        assert_eq!(first.strength, 0.45);
        // seed_mode increment applies in feed mode (noise auto = reroll).
        assert_eq!(first.seed, 7 + first.frame_index);
    }

    #[test]
    fn run_live_feedback_cold_starts_from_slot0_then_loops_with_the_source_as_anchor() {
        let source = gradient_image(32, 32);
        let reference = source.clone();
        let frames = run_recording_session(
            recording_params(LoopMode::Feedback),
            move |session| {
                // Nothing for a while: the worker must wait, not fail.
                std::thread::sleep(Duration::from_millis(120));
                session.set_reference(0, reference);
            },
            |frames| frames.len() >= 4,
            source,
        );
        let first = &frames[0];
        assert_eq!(first.frame_index, 0);
        assert!(!first.has_init, "cold start: no init");
        assert!(first.has_anchor && first.anchor_matches_source);
        assert_eq!(first.strength, 1.0, "cold start is one full edit");
        assert_eq!(first.references, 0, "slot 0 rides as the anchor, not as an extra reference");
        for frame in &frames[1..4] {
            assert!(frame.has_init, "warm frames carry the blended init");
            assert!(frame.has_anchor && frame.anchor_matches_source, "the anchor stays the untouched source");
            assert_eq!(frame.strength, 0.45);
            assert_eq!(frame.references, 0);
        }
        // noise auto = hold in feedback: seed_mode increment is NOT applied.
        assert!(frames.iter().all(|frame| frame.seed == 7), "{:?}", frames.iter().map(|f| f.seed).collect::<Vec<_>>());
    }

    #[test]
    fn run_live_feedback_takes_a_pushed_frame_as_source_and_reset_cold_starts_again() {
        let source = gradient_image(32, 32);
        let pushed = source.clone();
        let frames = run_recording_session(
            recording_params(LoopMode::Feedback),
            move |session| {
                session.push_input_frame(pushed);
                // Let a few frames run, then ask for a cold start.
                std::thread::sleep(Duration::from_millis(60));
                session.handle_text(r#"{"type":"control","reset":true}"#).unwrap();
            },
            |frames| frames.iter().filter(|f| !f.has_init).count() >= 2 && frames.len() >= 6,
            source,
        );
        assert!(!frames[0].has_init && frames[0].strength == 1.0);
        assert!(frames[0].anchor_matches_source, "a pushed frame is the feedback source");
        assert!(frames[1].has_init, "the frame after a cold start is a warm one");
        let second_cold = frames.iter().skip(1).position(|f| !f.has_init).expect("reset cold-starts again") + 1;
        assert_eq!(frames[second_cold].strength, 1.0);
        assert!(frames[second_cold + 1..].iter().all(|f| f.has_init), "one cold start per reset, not a stuck one");
    }

    #[test]
    fn run_live_feedback_moves_to_a_new_size_on_a_control_resize() {
        let source = gradient_image(32, 32);
        let pushed = source.clone();
        let frames = run_recording_session(
            recording_params(LoopMode::Feedback),
            move |session| {
                session.push_input_frame(pushed);
                std::thread::sleep(Duration::from_millis(60));
                session.handle_text(r#"{"type":"control","width":64,"height":48}"#).unwrap();
            },
            |frames| frames.iter().filter(|f| f.init_size == Some((64, 48))).count() >= 3,
            source,
        );
        assert_eq!(frames[1].init_size, Some((32, 32)));
        let moved = frames.iter().position(|f| f.init_size == Some((64, 48))).unwrap();
        // The loop carries on at the new size: no cold start, the anchor
        // follows, and every frame after the switch is the new size.
        assert!(frames[moved].has_init, "a resize is not a cold start");
        assert_eq!(frames[moved].anchor_size, Some((64, 48)));
        assert!(frames[moved..].iter().all(|f| f.init_size == Some((64, 48)) && f.anchor_size == Some((64, 48))));
    }

    #[test]
    fn session_mailbox_keeps_only_latest_and_counts_dropped() {
        let params = LiveParams {
            model: "testpattern".to_string(),
            config: LiveConfig::default(),
            loop_mode: LoopMode::Feed,
            input_encoding: OutputEncoding::Raw,
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

    /// A feed that moves from one box to another carries ONE thing with it:
    /// the trip so far. The receiving session takes that frame as its own
    /// previous output, so its next iteration continues the melt — instead
    /// of the cold start (one full edit from the clean picture) that a fresh
    /// session does, which on screen is a snap back to the engraving.
    #[test]
    fn a_seeded_session_carries_the_trip_instead_of_cold_starting() {
        let params = LiveParams {
            model: "testpattern".to_string(),
            config: LiveConfig::default(),
            loop_mode: LoopMode::Feedback,
            input_encoding: OutputEncoding::Raw,
            output_encoding: OutputEncoding::Raw,
            max_fps: 0.0,
            idle_timeout_s: 30,
        };
        let session = RealtimeSession::new("job-seed".to_string(), &params);
        // Nothing to carry until something is sent.
        assert!(session.take_seed_output().is_none());

        let trip = solid_image(8, 8, [10, 200, 40]);
        let raw_b64 = String::from_utf8(makepad_base64::base64_encode(&trip.data, &makepad_base64::BASE64_STANDARD)).unwrap();
        let text = format!("{{\"type\":\"seed_output\",\"raw_b64\":\"{raw_b64}\",\"w\":8,\"h\":8}}");
        session.handle_text(&text).unwrap();
        let carried = session.take_seed_output().expect("the trip arrived");
        assert_eq!(carried.data, trip.data);
        // Taken once — it is this session's own previous output from here on.
        assert!(session.take_seed_output().is_none());

        // And what the loop does with it: WITH the trip there is an init, so
        // the sampler continues from it; WITHOUT one there is none, which is
        // the cold start that forces a full-strength repaint of the source.
        let source = gradient_image(8, 8);
        let stats = channel_stats(&source);
        let mut config = blank_config();
        config.drift = identity_drift();
        config.camera = CameraMotion::default();
        config.feedback = 0.7;
        assert!(feedback_frame(&source, &stats, None, &config, 0).is_none(), "no trip = cold start");
        let init = feedback_frame(&source, &stats, Some(&carried), &config, 1).expect("the trip is the init");
        let to_seed = mean_abs_diff(&init, &carried).unwrap();
        let to_source = mean_abs_diff(&init, &source).unwrap();
        assert!(
            to_seed < to_source * 0.6,
            "the first frame after a move must continue the trip, not repaint the picture: {to_seed} to the trip vs {to_source} to the source"
        );

        // A seed that does not describe an image is refused rather than
        // quietly becoming a smear.
        assert!(session.handle_text(r#"{"type":"seed_output","raw_b64":"AAAA","w":8,"h":8}"#).is_err());
        assert!(session.handle_text(r#"{"type":"seed_output"}"#).is_err());
    }

    #[test]
    fn session_apply_control_updates_session_only_knobs() {
        let params = LiveParams {
            model: "testpattern".to_string(),
            config: LiveConfig::default(),
            loop_mode: LoopMode::Feed,
            input_encoding: OutputEncoding::Raw,
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
        session.apply_control(&update).unwrap();
        let (config, loop_mode, output_encoding, max_fps) = session.snapshot();
        assert_eq!(loop_mode, LoopMode::Feedback);
        assert_eq!(output_encoding, OutputEncoding::Png);
        assert_eq!(max_fps, 24.0);
        assert_eq!(config.prompt, "hi");
    }

    #[test]
    fn none_control_sends_no_frame_and_refuses_feedback() {
        let params = LiveParams {
            model: "testpattern".to_string(),
            config: LiveConfig::default(),
            loop_mode: LoopMode::Feed,
            input_encoding: OutputEncoding::Raw,
            output_encoding: OutputEncoding::None,
            max_fps: 0.0,
            idle_timeout_s: 30,
        };
        let session = RealtimeSession::new("job-none".to_string(), &params);
        assert!(session.encode_output(&RgbImage::blank(16, 16), 0).is_empty());

        let update = ControlUpdateJson {
            kind: "control".to_string(),
            loop_mode: Some("feedback".to_string()),
            ..Default::default()
        };
        let error = session.apply_control(&update).unwrap_err();
        assert!(matches!(error, AssetAiError::Params(_)));
        let (_, loop_mode, output_encoding, _) = session.snapshot();
        assert_eq!(loop_mode, LoopMode::Feed);
        assert_eq!(output_encoding, OutputEncoding::None);
    }

    /// The server-loop handshake: open in feed, push the source once, flip
    /// to feedback after the first output. The worker used to park on the
    /// mailbox inside the feed branch and never re-read the mode — one frame
    /// out, then silence for ever (the user's "it just says starting").
    #[test]
    fn run_live_flips_from_feed_to_feedback_while_waiting_for_input() {
        let source = gradient_image(32, 32);
        let params = recording_params(LoopMode::Feed);
        let session = std::sync::Arc::new(RealtimeSession::new("job-t".to_string(), &params));
        let (socket_tx, _socket_rx) = mpsc::channel();
        session.add_socket(1, socket_tx);
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let worker = {
            let session = session.clone();
            let seen = seen.clone();
            let source = source.clone();
            std::thread::spawn(move || {
                let mut backend = RecordingBackend { seen, source };
                let cancel = CancelToken::new();
                run_live(&session, &mut backend, &cancel, |_, _, _, _| {})
            })
        };
        // One pushed source, one feed frame out.
        session.push_input_frame(source.clone());
        let deadline = Instant::now() + Duration::from_secs(10);
        while seen.lock().unwrap().len() < 1 {
            assert!(Instant::now() < deadline, "no feed frame");
            std::thread::sleep(Duration::from_millis(5));
        }
        // The flip arrives while the worker waits for input it will never
        // get. It must free-run from here without another pushed frame.
        session.handle_text(r#"{"type":"control","loop_mode":"feedback"}"#).unwrap();
        while seen.lock().unwrap().len() < 5 {
            assert!(Instant::now() < deadline, "the flip to feedback never freed the loop");
            std::thread::sleep(Duration::from_millis(5));
        }
        session.request_stop();
        worker.join().unwrap().unwrap();
        assert_eq!(session.frames_in.load(Ordering::Relaxed), 1, "feedback frames need no input");
        let frames = seen.lock().unwrap().clone();
        // Feedback frames anchor on the source the feed frame set.
        assert!(frames[2].has_anchor && frames[2].anchor_matches_source);
    }

    /// A feedback loop with no listener holds instead of painting into the
    /// void; a socket arriving resumes it.
    #[test]
    fn run_live_feedback_holds_without_a_listener_and_resumes_on_attach() {
        let source = gradient_image(32, 32);
        let params = recording_params(LoopMode::Feedback);
        let session = std::sync::Arc::new(RealtimeSession::new("job-t".to_string(), &params));
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let worker = {
            let session = session.clone();
            let seen = seen.clone();
            let source = source.clone();
            std::thread::spawn(move || {
                let mut backend = RecordingBackend { seen, source };
                let cancel = CancelToken::new();
                run_live(&session, &mut backend, &cancel, |_, _, _, _| {})
            })
        };
        session.set_reference(0, source.clone());
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(seen.lock().unwrap().len(), 0, "generated with nobody listening");
        let (socket_tx, _socket_rx) = mpsc::channel();
        session.add_socket(1, socket_tx);
        let deadline = Instant::now() + Duration::from_secs(10);
        while seen.lock().unwrap().len() < 3 {
            assert!(Instant::now() < deadline, "never resumed after the socket attached");
            std::thread::sleep(Duration::from_millis(5));
        }
        session.request_stop();
        worker.join().unwrap().unwrap();
    }
}
