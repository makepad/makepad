//! One HEVC intra frame through the phone's codecs (NDK AMediaCodec): an NV12 still encoded to
//! its parameter sets and access unit, and one access unit decoded back to NV12. What the image
//! tile libraries keep on disk (`makepad_video::encode_intra_frame_mp4` /
//! `decode_first_frame_from_bytes`); the program reaches it through the `hal_hevc_*` ops.
//!
//! The encoder takes semi-planar YUV (COLOR_FormatYUV420SemiPlanar = NV12) at the stride and
//! slice height its input format names. The decoder renders into an `AImageReader` of
//! YUV_420_888: a hardware decoder's own buffer layout is vendor-private, the reader's planes are
//! not. Each call makes and frees its codec, so any thread may call and calls never share state.
use super::mediacodec::{checked, ndk, BufferInfo, Codec, Format, Reader, FLAG_CODEC_CONFIG, FLAG_END_OF_STREAM};
use super::{exception, java, method};
use makepad_jni_sys::*;
use std::ffi::{c_void, CStr};
use std::time::{Duration, Instant};

const MIME: &CStr = c"video/hevc";
/// MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420SemiPlanar.
const COLOR_NV12: i32 = 21;
const CONFIGURE_ENCODE: u32 = 1;
const TIMEOUT: Duration = Duration::from_secs(5);

/// The largest frame (width, height) the phone's HEVC encoder takes, hardware first
/// (`MediaCodecInfo.VideoCapabilities`, through the Activity). `None` when the phone has none.
pub fn encoder_max_size() -> Option<(u32, u32)> {
    let size = java(|env, activity| unsafe {
        let id = method(env, activity, c"hevcEncoderMaxSize", c"(I)I")?;
        let width = ((**env).CallIntMethodA.unwrap())(env, activity, id, [jvalue { i: 0 }].as_ptr());
        exception(env)?;
        let height = ((**env).CallIntMethodA.unwrap())(env, activity, id, [jvalue { i: 1 }].as_ptr());
        exception(env)?;
        Ok((width, height))
    });
    match size {
        Ok((w, h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
        Ok(_) => None,
        Err(e) => {
            eprintln!("[hal-android-hevc] encoder size query: {e}");
            None
        }
    }
}

/// An NV12 still (`width * height` luma, then interleaved chroma) as the encoder's parameter
/// sets (VPS/SPS/PPS) and the frame's access unit, both Annex-B.
pub fn encode(nv12: &[u8], width: u32, height: u32, bitrate: u32) -> Result<(Vec<u8>, Vec<u8>), String> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 || nv12.len() < w * h * 3 / 2 {
        return Err(format!("invalid {width}x{height} NV12 still ({} bytes)", nv12.len()));
    }
    let codec = Codec::encoder(MIME)?;
    let format = Format::new();
    format.string(c"mime", MIME);
    format.int(c"width", width as i32);
    format.int(c"height", height as i32);
    format.int(c"color-format", COLOR_NV12);
    format.int(c"bitrate", bitrate.max(100_000) as i32);
    format.int(c"frame-rate", 30);
    format.int(c"i-frame-interval", 0);
    unsafe {
        checked(ndk::AMediaCodec_configure(codec.0, format.0, std::ptr::null_mut(), std::ptr::null_mut(), CONFIGURE_ENCODE), "configure the encoder")?;
        checked(ndk::AMediaCodec_start(codec.0), "start the encoder")?;
        // The layout the encoder wants its input in.
        let input = Format(ndk::AMediaCodec_getInputFormat(codec.0));
        let stride = input.get_int(c"stride").filter(|s| *s as usize >= w).unwrap_or(width as i32) as usize;
        let slice = input.get_int(c"slice-height").filter(|s| *s as usize >= h).unwrap_or(height as i32) as usize;
        let index = ndk::AMediaCodec_dequeueInputBuffer(codec.0, TIMEOUT.as_micros() as i64);
        if index < 0 {
            return Err(format!("the encoder gave no input buffer ({index})"));
        }
        let mut capacity = 0usize;
        let buffer = ndk::AMediaCodec_getInputBuffer(codec.0, index as usize, &mut capacity);
        let needed = stride * slice + stride * h / 2;
        if buffer.is_null() || capacity < needed {
            return Err(format!("the encoder's input buffer holds {capacity} bytes, the still needs {needed}"));
        }
        let out = std::slice::from_raw_parts_mut(buffer, capacity);
        for row in 0..h {
            out[row * stride..row * stride + w].copy_from_slice(&nv12[row * w..row * w + w]);
        }
        let chroma = &nv12[w * h..];
        for row in 0..h / 2 {
            let at = stride * slice + row * stride;
            out[at..at + w].copy_from_slice(&chroma[row * w..row * w + w]);
        }
        checked(ndk::AMediaCodec_queueInputBuffer(codec.0, index as usize, 0, needed, 0, FLAG_END_OF_STREAM), "queue the still")?;
        let (mut config, mut frame) = (Vec::new(), Vec::new());
        drain(codec.0, |flags, bytes| {
            if flags & FLAG_CODEC_CONFIG != 0 { config.extend_from_slice(bytes) } else { frame.extend_from_slice(bytes) }
        })?;
        if frame.is_empty() {
            return Err("the encoder gave no frame".into());
        }
        Ok((config, frame))
    }
}

/// One Annex-B access unit (parameter sets in band) of a `width` x `height` picture as NV12.
pub fn decode(annex_b: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(format!("invalid frame size {width}x{height}"));
    }
    let (reader, window) = Reader::yuv(width, height, 2)?;
    let codec = Codec::decoder(MIME)?;
    let format = Format::new();
    format.string(c"mime", MIME);
    format.int(c"width", width as i32);
    format.int(c"height", height as i32);
    format.int(c"max-input-size", annex_b.len() as i32 + 1024);
    unsafe {
        checked(ndk::AMediaCodec_configure(codec.0, format.0, window, std::ptr::null_mut(), 0), "configure the decoder")?;
        checked(ndk::AMediaCodec_start(codec.0), "start the decoder")?;
        let index = ndk::AMediaCodec_dequeueInputBuffer(codec.0, TIMEOUT.as_micros() as i64);
        if index < 0 {
            return Err(format!("the decoder gave no input buffer ({index})"));
        }
        let mut capacity = 0usize;
        let buffer = ndk::AMediaCodec_getInputBuffer(codec.0, index as usize, &mut capacity);
        if buffer.is_null() || capacity < annex_b.len() {
            return Err(format!("the decoder's input buffer holds {capacity} bytes, the frame is {}", annex_b.len()));
        }
        std::slice::from_raw_parts_mut(buffer, annex_b.len()).copy_from_slice(annex_b);
        checked(ndk::AMediaCodec_queueInputBuffer(codec.0, index as usize, 0, annex_b.len(), 0, FLAG_END_OF_STREAM), "queue the frame")?;
        let start = Instant::now();
        loop {
            let mut info = BufferInfo::default();
            let index = ndk::AMediaCodec_dequeueOutputBuffer(codec.0, &mut info, 50_000);
            if index >= 0 {
                let picture = info.size > 0 && info.flags & FLAG_CODEC_CONFIG == 0;
                ndk::AMediaCodec_releaseOutputBuffer(codec.0, index as usize, picture);
                if picture {
                    break;
                }
                if info.flags & FLAG_END_OF_STREAM != 0 {
                    return Err("the decoder ended without a picture".into());
                }
            } else if start.elapsed() > TIMEOUT {
                return Err("the decoder gave no picture".into());
            }
        }
    }
    // The picture reaches the reader a moment after its release.
    reader.next_nv12(width, height, TIMEOUT)
}

/// Every output buffer until the end of stream, each to `take(flags, bytes)`.
unsafe fn drain(codec: *mut c_void, mut take: impl FnMut(u32, &[u8])) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let mut info = BufferInfo::default();
        let index = ndk::AMediaCodec_dequeueOutputBuffer(codec, &mut info, 50_000);
        if index >= 0 {
            let mut capacity = 0usize;
            let buffer = ndk::AMediaCodec_getOutputBuffer(codec, index as usize, &mut capacity);
            if !buffer.is_null() && info.size > 0 && (info.offset + info.size) as usize <= capacity {
                take(info.flags, std::slice::from_raw_parts(buffer.add(info.offset as usize), info.size as usize));
            }
            ndk::AMediaCodec_releaseOutputBuffer(codec, index as usize, false);
            if info.flags & FLAG_END_OF_STREAM != 0 {
                return Ok(());
            }
        } else if start.elapsed() > TIMEOUT {
            return Err("the encoder did not finish".into());
        }
    }
}
