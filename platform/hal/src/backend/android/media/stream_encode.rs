//! The phone's hardware stream encoder (NDK AMediaCodec): what `crate::stream_encode` drives on
//! Android, the realtime sibling of `hevc` (single stills). H.264 (or H.265), NV12 pictures in
//! through ByteBuffers (COLOR_FormatYUV420SemiPlanar, at the stride and slice height the codec's
//! input format names), Annex-B access units out with the codec's config bytes (SPS/PPS, VPS for
//! H.265) in front of every key frame, as the Desk Link wire wants (apps/desklink/PROTOCOL.md
//! "Video").
//!
//! Low latency: priority 0 (realtime), `low-latency` 1, no B-frames, key frames only on request
//! (`request-sync`; the i-frame interval is an hour). Each picture is synchronous: the call waits
//! for its access unit and returns one access unit. An encoder that keeps one picture back (some
//! vendors' do, whatever `low-latency` says) is found on the first picture that times out; from
//! then on each call returns the picture before it, a frame of latency instead of a timeout every
//! frame (and the stream's first call returns nothing).
use super::mediacodec::{checked, ndk, BufferInfo, Codec as MediaCodec, Format, FLAG_CODEC_CONFIG, FLAG_END_OF_STREAM, OUTPUT_FORMAT_CHANGED};
use crate::stream_pack::{key_unit, nv12_to_strided, strided_nv12_len};
use std::ffi::CStr;
use std::time::{Duration, Instant};

/// MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420SemiPlanar.
const COLOR_NV12: i32 = 21;
const CONFIGURE_ENCODE: u32 = 1;
/// MediaCodec.BUFFER_FLAG_KEY_FRAME.
const FLAG_KEY_FRAME: u32 = 1;
/// MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR / _CBR.
const BITRATE_VBR: i32 = 1;
const BITRATE_CBR: i32 = 2;
/// Seconds between scheduled key frames: none in practice, key frames come on request.
const KEY_INTERVAL_S: i32 = 3600;
/// How long a picture's access unit is waited for; a picture that takes longer was kept back
/// by the encoder (see the module doc).
const PICTURE_WAIT: Duration = Duration::from_millis(80);
/// The first picture's wait (an encoder's first picture may take longer).
const FIRST_PICTURE_WAIT: Duration = Duration::from_millis(500);
/// How long an input buffer is waited for.
const INPUT_WAIT: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
}

/// How a stream trades bits and latency (the fields Linux's encoder takes; MediaCodec uses the
/// rate mode).
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    pub quality: u32,
    pub constant: bool,
    pub buffer_pictures: u32,
    pub sharp: bool,
}

pub struct StreamEncoder {
    codec: MediaCodec,
    hevc: bool,
    width: u32,
    height: u32,
    stride: u32,
    slice_height: u32,
    fps: u32,
    /// The config bytes the codec gave (parameter sets, Annex-B), put in front of each key frame.
    config: Vec<u8>,
    /// Pictures queued and access units received.
    queued: u64,
    received: u64,
    /// Access units the encoder keeps back (0, or 1 once it was seen to).
    lag: u64,
    pub info: String,
}

/// A rate for the picture size when none is asked for: 0.1 bits a pixel a frame.
fn default_kbps(width: u32, height: u32, fps: u32) -> u32 {
    ((width as u64 * height as u64 * fps as u64) / 10_000).clamp(500, 50_000) as u32
}

fn set_parameter(codec: &MediaCodec, key: &CStr, value: i32) -> Result<(), String> {
    let params = Format::new();
    params.int(key, value);
    checked(unsafe { ndk::AMediaCodec_setParameters(codec.0, params.0) }, &format!("set {}", key.to_string_lossy()))
}

impl StreamEncoder {
    pub fn open(width: u32, height: u32, fps: u32, kbps: u32, codec: Codec, tuning: Tuning) -> Result<StreamEncoder, String> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 || fps == 0 {
            return Err(format!("invalid stream {width}x{height} at {fps} fps"));
        }
        let hevc = codec == Codec::H265;
        let mime: &CStr = if hevc { c"video/hevc" } else { c"video/avc" };
        let kbps = if kbps == 0 { default_kbps(width, height, fps) } else { kbps };
        let media = MediaCodec::encoder(mime)?;
        let format = Format::new();
        format.string(c"mime", mime);
        format.int(c"width", width as i32);
        format.int(c"height", height as i32);
        format.int(c"color-format", COLOR_NV12);
        format.int(c"bitrate", (kbps.saturating_mul(1000)).min(i32::MAX as u32) as i32);
        format.int(c"bitrate-mode", if tuning.constant { BITRATE_CBR } else { BITRATE_VBR });
        format.int(c"frame-rate", fps as i32);
        format.int(c"i-frame-interval", KEY_INTERVAL_S);
        format.int(c"priority", 0);
        format.int(c"low-latency", 1);
        format.int(c"max-bframes", 0);
        unsafe {
            checked(ndk::AMediaCodec_configure(media.0, format.0, std::ptr::null_mut(), std::ptr::null_mut(), CONFIGURE_ENCODE), "configure the stream encoder")?;
            checked(ndk::AMediaCodec_start(media.0), "start the stream encoder")?;
        }
        // the layout the encoder wants its input in
        let input = Format(unsafe { ndk::AMediaCodec_getInputFormat(media.0) });
        let stride = input.get_int(c"stride").filter(|s| *s as u32 >= width).map_or(width, |s| s as u32);
        let slice_height = input.get_int(c"slice-height").filter(|s| *s as u32 >= height).map_or(height, |s| s as u32);
        let name = unsafe {
            let mut name = std::ptr::null_mut();
            if ndk::AMediaCodec_getName(media.0, &mut name) == 0 && !name.is_null() {
                let text = CStr::from_ptr(name).to_string_lossy().into_owned();
                ndk::AMediaCodec_releaseName(media.0, name);
                text
            } else {
                "MediaCodec".to_string()
            }
        };
        let info = format!("{name} {} {width}x{height} {fps} fps {kbps} kbit/s {} (input stride {stride}, slice height {slice_height})",
            if hevc { "H.265" } else { "H.264" }, if tuning.constant { "CBR" } else { "VBR" });
        Ok(StreamEncoder { codec: media, hevc, width, height, stride, slice_height, fps, config: Vec::new(), queued: 0, received: 0, lag: 0, info })
    }

    /// The rate from the next picture on (kbit/s).
    pub fn set_kbps(&mut self, kbps: u32) {
        let bits = (kbps.max(1).saturating_mul(1000)).min(i32::MAX as u32) as i32;
        if let Err(e) = set_parameter(&self.codec, c"video-bitrate", bits) {
            eprintln!("[hal-android-stream] {e}");
        }
    }

    /// GPU time is not measured on this encoder.
    pub fn last_gpu_ns(&self) -> u64 {
        0
    }

    pub fn note_failure(&self, e: &str) {
        eprintln!("[hal-android-stream] {}: {e}", self.info);
    }

    /// Encodes one tightly packed NV12 picture (a key frame when `key`): one access unit, Annex-B,
    /// parameter sets in front of a key frame. From an encoder that keeps a picture back it is
    /// the picture before (empty for the stream's first).
    pub fn encode_nv12(&mut self, nv12: &[u8], key: bool) -> Result<Vec<u8>, String> {
        let (w, h) = (self.width, self.height);
        if nv12.len() != w as usize * h as usize * 3 / 2 {
            return Err(format!("{} bytes are no {w}x{h} NV12 picture", nv12.len()));
        }
        if key {
            set_parameter(&self.codec, c"request-sync", 0)?;
        }
        let mut out = Vec::new();
        let index = self.input_buffer(&mut out)?;
        unsafe {
            let mut capacity = 0usize;
            let buffer = ndk::AMediaCodec_getInputBuffer(self.codec.0, index, &mut capacity);
            if buffer.is_null() {
                return Err("the encoder's input buffer is missing".into());
            }
            let needed = strided_nv12_len(h, self.stride, self.slice_height);
            if capacity < needed {
                return Err(format!("the encoder's input buffer holds {capacity} bytes, the picture needs {needed}"));
            }
            let len = nv12_to_strided(nv12, w, h, self.stride, self.slice_height, std::slice::from_raw_parts_mut(buffer, capacity))?;
            let time_us = self.queued * 1_000_000 / self.fps as u64;
            checked(ndk::AMediaCodec_queueInputBuffer(self.codec.0, index, 0, len, time_us, 0), "queue the picture")?;
        }
        self.queued += 1;
        // this picture's access unit (or the one before it, from an encoder that keeps one back);
        // more stay in the codec for the next calls: one access unit a call
        let wait = if self.queued == 1 { FIRST_PICTURE_WAIT } else { PICTURE_WAIT };
        let start = Instant::now();
        while self.received + self.lag < self.queued {
            let left = wait.saturating_sub(start.elapsed());
            if left.is_zero() {
                if self.lag == 0 {
                    self.lag = 1;
                    eprintln!("[hal-android-stream] {}: the encoder keeps a picture back (a frame of latency)", self.info);
                    continue;
                }
                return Err(format!("the encoder gave no picture within {} ms", wait.as_millis()));
            }
            self.drain(&mut out, left, (self.queued - self.lag - self.received) as usize)?;
        }
        Ok(out)
    }

    /// An input buffer, draining output meanwhile (an encoder with all its inputs queued frees
    /// one once its output is taken).
    fn input_buffer(&mut self, out: &mut Vec<u8>) -> Result<usize, String> {
        let start = Instant::now();
        loop {
            let index = unsafe { ndk::AMediaCodec_dequeueInputBuffer(self.codec.0, 5_000) };
            if index >= 0 {
                return Ok(index as usize);
            }
            if start.elapsed() > INPUT_WAIT {
                return Err(format!("the encoder gave no input buffer ({index})"));
            }
            // (one access unit a call: none was taken yet when the inputs are all busy)
            let pictures = usize::from(out.is_empty());
            self.drain(out, Duration::from_millis(5), pictures)?;
            if pictures == 0 {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    /// Output buffers into `out` until `pictures` access units came (config buffers do not
    /// count) or none is ready within `wait`.
    fn drain(&mut self, out: &mut Vec<u8>, wait: Duration, pictures: usize) -> Result<(), String> {
        let timeout_us = wait.as_micros() as i64;
        let mut got = 0;
        while got < pictures {
            let mut info = BufferInfo::default();
            let index = unsafe { ndk::AMediaCodec_dequeueOutputBuffer(self.codec.0, &mut info, timeout_us) };
            if index == OUTPUT_FORMAT_CHANGED || index == OUTPUT_BUFFERS_CHANGED {
                continue;
            }
            if index < 0 {
                return Ok(());
            }
            let bytes = unsafe {
                let mut capacity = 0usize;
                let buffer = ndk::AMediaCodec_getOutputBuffer(self.codec.0, index as usize, &mut capacity);
                if !buffer.is_null() && info.size > 0 && info.offset >= 0 && (info.offset + info.size) as usize <= capacity {
                    std::slice::from_raw_parts(buffer.add(info.offset as usize), info.size as usize).to_vec()
                } else {
                    Vec::new()
                }
            };
            unsafe { ndk::AMediaCodec_releaseOutputBuffer(self.codec.0, index as usize, false) };
            if info.flags & FLAG_CODEC_CONFIG != 0 {
                self.config = bytes;
            } else if !bytes.is_empty() {
                self.received += 1;
                got += 1;
                if info.flags & FLAG_KEY_FRAME != 0 {
                    out.extend_from_slice(&key_unit(&self.config, &bytes, self.hevc));
                } else {
                    out.extend_from_slice(&bytes);
                }
            }
            if info.flags & FLAG_END_OF_STREAM != 0 {
                return Err("the encoder ended the stream".into());
            }
        }
        Ok(())
    }
}

/// AMediaCodec_dequeueOutputBuffer's "the output buffers changed" (deprecated; nothing to do).
const OUTPUT_BUFFERS_CHANGED: isize = -3;
