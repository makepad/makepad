//! Windows backend for the low-latency stream decoder: the Microsoft
//! H.264 decoder MFT (`CLSID_CMSH264DecoderMFT`), driven directly via
//! `IMFTransform::ProcessInput`/`ProcessOutput` (see [`crate::windows_
//! mft`]).
//!
//! No DXVA/hardware surface path — output is read back to system memory
//! NV12 via `IMFMediaBuffer::Lock`, same as the file-based decoder.
//!
//! Lifecycle that the live decoder needs, in order:
//! 1. `MF_LOW_LATENCY` on the transform attributes — otherwise the decoder
//!    holds a reorder window and a 2–3 frame live pipeline never sees a
//!    single output frame.
//! 2. Input type = bare H.264 (no frame-size hint; the SPS decides).
//! 3. An output type committed BEFORE the first `ProcessOutput` — the
//!    decoder offers NV12 (at a placeholder size) as soon as the input type
//!    is set, and `ProcessOutput` without one is `MF_E_TRANSFORM_TYPE_NOT_
//!    SET`, not a stream change.
//! 4. `ProcessOutput` reports `MF_E_TRANSFORM_STREAM_CHANGE` once the real
//!    picture size is known; re-negotiate and retry.
//!
//! `MAKEPAD_H264_DEBUG=<file>` traces every step (see [`crate::stream_debug`]).

use crate::stream_debug::{self as dbg, hex_hr};
use crate::stream_decoder::DecodedFrame;
use crate::stream_encoder::StreamVideoCodec;
use crate::windows_encoder::{ensure_media_foundation, hr_err};
use crate::windows_mft::{
    self, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    CODECAPI_AV_LOW_LATENCY_MODE, MF_E_BUFFERTOOSMALL, MF_E_NOTACCEPTING, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_E_TRANSFORM_TYPE_NOT_SET, MF_LOW_LATENCY,
};
use crate::VideoFileError;
use std::collections::VecDeque;
use windows::{
    core::GUID,
    Win32::Media::MediaFoundation::{
        IMFMediaType, IMFSample, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
        MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_NV12, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE,
        MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    },
    Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
};

/// Well-known Microsoft H.264 decoder MFT CLSID (msmpeg2vdec.dll) — stable
/// and documented since Windows 7; not in the vendored bindings.
const CLSID_CMS_H264_DECODER_MFT: GUID = GUID::from_u128(0x62ce7e72_4c71_4d20_b15d_452831a87d9d);

/// Output allocation floor while the decoder still reports a placeholder
/// size (1080p NV12): the first real picture must fit before the stream
/// change that tells us its size has been processed.
const OUTPUT_BUFFER_FLOOR: u32 = 1920 * 1088 * 3 / 2;

/// How many consecutive type re-negotiations one `ProcessOutput` pump may
/// do before giving up — a decoder that keeps flipping types is broken.
const MAX_RENEGOTIATIONS: u32 = 4;

fn create_transform() -> Result<IMFTransform, VideoFileError> {
    ensure_media_foundation()?;
    unsafe { CoCreateInstance(&CLSID_CMS_H264_DECODER_MFT, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| hr_err("CoCreateInstance(CLSID_CMSH264DecoderMFT)", e))
}

fn make_input_sample(annex_b: &[u8], pts_100ns: i64) -> Result<IMFSample, VideoFileError> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(annex_b.len().max(1) as u32).map_err(|e| hr_err("MFCreateMemoryBuffer", e))?;
        let mut ptr = std::ptr::null_mut();
        buffer.Lock(&mut ptr, None, None).map_err(|e| hr_err("Lock", e))?;
        std::ptr::copy_nonoverlapping(annex_b.as_ptr(), ptr, annex_b.len());
        buffer.Unlock().map_err(|e| hr_err("Unlock", e))?;
        buffer.SetCurrentLength(annex_b.len() as u32).map_err(|e| hr_err("SetCurrentLength", e))?;
        let sample = MFCreateSample().map_err(|e| hr_err("MFCreateSample", e))?;
        sample.AddBuffer(&buffer).map_err(|e| hr_err("AddBuffer", e))?;
        sample.SetSampleTime(pts_100ns).map_err(|e| hr_err("SetSampleTime", e))?;
        Ok(sample)
    }
}

fn make_output_sample(size: u32) -> Result<IMFSample, VideoFileError> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(size.max(1)).map_err(|e| hr_err("MFCreateMemoryBuffer(out)", e))?;
        let sample = MFCreateSample().map_err(|e| hr_err("MFCreateSample(out)", e))?;
        sample.AddBuffer(&buffer).map_err(|e| hr_err("AddBuffer(out)", e))?;
        Ok(sample)
    }
}

/// `(width, height)` from a media type's `MF_MT_FRAME_SIZE`, `None` while
/// the decoder still carries a placeholder (absent or zero) size.
unsafe fn frame_size_of(media_type: &IMFMediaType) -> Option<(u32, u32)> {
    let packed = media_type.GetUINT64(&MF_MT_FRAME_SIZE).ok()?;
    let size = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
    (size.0 > 0 && size.1 > 0).then_some(size)
}

pub struct WindowsStreamDecoder {
    transform: IMFTransform,
    provides_samples: bool,
    output_buffer_size: u32,
    width: u32,
    height: u32,
    output_stride: usize,
    pending_pts: VecDeque<i64>,
    packets_in: u64,
    frames_out: u64,
    pumps: u64,
}

impl WindowsStreamDecoder {
    pub fn new(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        if !matches!(codec, StreamVideoCodec::H264) {
            return Err(VideoFileError::new("windows stream decoder: only H264 is implemented"));
        }
        let transform = create_transform()?;
        unsafe {
            match windows_mft::get_attributes(&transform) {
                Ok(attributes) => {
                    let set = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
                    dbg::log(|| format!("h264dec: MF_LOW_LATENCY set -> {set:?}"));
                }
                Err(e) => dbg::log(|| format!("h264dec: GetAttributes failed {e:?} (no low-latency mode)")),
            }
            let codec_api = windows_mft::set_codec_api_bool(&transform, &CODECAPI_AV_LOW_LATENCY_MODE, true);
            dbg::log(|| format!("h264dec: CODECAPI_AVLowLatencyMode set -> {codec_api:?}"));
            let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(in)", e))?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| hr_err("set in major type", e))?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| hr_err("set in subtype", e))?;
            windows_mft::set_input_type(&transform, &in_type).map_err(|e| hr_err("IMFTransform::SetInputType", e))?;
        }
        let mut decoder = Self {
            transform,
            provides_samples: false,
            output_buffer_size: OUTPUT_BUFFER_FLOOR,
            width: 0,
            height: 0,
            output_stride: 0,
            pending_pts: VecDeque::new(),
            packets_in: 0,
            frames_out: 0,
            pumps: 0,
        };
        // Committing an output type up front is what makes the first
        // ProcessOutput answer NEED_MORE_INPUT / STREAM_CHANGE instead of
        // TYPE_NOT_SET; a decoder that refuses this early is still driven
        // through the TYPE_NOT_SET branch of the pump.
        if let Err(e) = decoder.negotiate_output_type() {
            dbg::log(|| format!("h264dec: eager output negotiation failed: {e}"));
        }
        unsafe {
            windows_mft::process_message(&decoder.transform, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| hr_err("ProcessMessage(BEGIN_STREAMING)", e))?;
            windows_mft::process_message(&decoder.transform, MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| hr_err("ProcessMessage(START_OF_STREAM)", e))?;
        }
        dbg::log(|| "h264dec: ready (input H264, streaming notified)".to_string());
        Ok(decoder)
    }

    /// Walks `GetOutputAvailableType` for an NV12 entry and commits it with
    /// `SetOutputType`, then refreshes size, stride, allocation mode and
    /// buffer size from the committed type. Called eagerly at construction
    /// (placeholder size) and again on every stream change (real size).
    fn negotiate_output_type(&mut self) -> Result<(), VideoFileError> {
        unsafe {
            let mut chosen = None;
            let mut offered = Vec::new();
            for index in 0.. {
                let Ok(candidate) = windows_mft::get_output_available_type(&self.transform, index) else {
                    break;
                };
                let subtype = candidate.GetGUID(&MF_MT_SUBTYPE).ok();
                offered.push(subtype.map(|g| format!("{g:?}")).unwrap_or_else(|| "?".into()));
                if subtype == Some(MFVideoFormat_NV12) && chosen.is_none() {
                    chosen = Some(candidate);
                }
            }
            dbg::log(|| format!("h264dec: output types offered: {} (nv12 {})", offered.join(", "), chosen.is_some()));
            let Some(chosen_type) = chosen else {
                return Err(VideoFileError::new(
                    "windows stream decoder: no NV12 output type offered by CMSH264DecoderMFT",
                ));
            };
            windows_mft::set_output_type(&self.transform, &chosen_type).map_err(|e| hr_err("SetOutputType(NV12)", e))?;
            self.refresh_output_geometry(&chosen_type)?;
        }
        Ok(())
    }

    unsafe fn refresh_output_geometry(&mut self, committed: &IMFMediaType) -> Result<(), VideoFileError> {
        if let Some((width, height)) = frame_size_of(committed) {
            self.width = width;
            self.height = height;
            self.output_stride = committed
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|stride| (stride as i32).unsigned_abs() as usize)
                .unwrap_or(width as usize)
                .max(width as usize);
        }
        let stream_info =
            windows_mft::get_output_stream_info(&self.transform).map_err(|e| hr_err("GetOutputStreamInfo", e))?;
        self.provides_samples = stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES != 0;
        let needed = (self.output_stride as u32).saturating_mul(self.height.div_ceil(2) * 3);
        self.output_buffer_size = stream_info.cbSize.max(needed).max(OUTPUT_BUFFER_FLOOR);
        dbg::log(|| {
            format!(
                "h264dec: output committed {}x{} stride {} provides_samples {} cbSize {} -> alloc {}",
                self.width, self.height, self.output_stride, self.provides_samples, stream_info.cbSize, self.output_buffer_size
            )
        });
        Ok(())
    }

    pub fn push_packet(&mut self, annex_b_data: &[u8], pts_100ns: i64) -> Result<Vec<DecodedFrame>, VideoFileError> {
        // See `WindowsStreamEncoder::push_frame_nv12`'s identical comment —
        // this type is `Send` and may be called from a different thread
        // than the one that constructed it; re-assert MTA membership here.
        ensure_media_foundation()?;
        dbg::dump_packet(self.packets_in, annex_b_data);
        self.packets_in += 1;
        let sample = make_input_sample(annex_b_data, pts_100ns)?;
        let mut hr = unsafe { windows_mft::process_input(&self.transform, &sample) };
        let mut frames = Vec::new();
        if hr.0 == MF_E_NOTACCEPTING {
            frames = self.drain_available()?;
            hr = unsafe { windows_mft::process_input(&self.transform, &sample) };
        }
        dbg::log(|| {
            format!(
                "h264dec: packet #{} {} bytes head [{}] pts {} ProcessInput {}",
                self.packets_in,
                annex_b_data.len(),
                dbg::head(annex_b_data),
                pts_100ns,
                hex_hr(hr.0)
            )
        });
        if hr.is_err() {
            return Err(VideoFileError::with_code("IMFTransform::ProcessInput", hr.0));
        }
        self.pending_pts.push_back(pts_100ns);
        frames.extend(self.drain_available()?);
        Ok(frames)
    }

    /// Drives the pull side until the decoder asks for more input. Type
    /// errors re-negotiate and retry; a too-small caller buffer re-reads
    /// the stream info and retries; every produced sample becomes a frame.
    fn drain_available(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        let mut frames = Vec::new();
        let mut renegotiations = 0;
        loop {
            let provided = if self.provides_samples { None } else { Some(make_output_sample(self.output_buffer_size)?) };
            let (hr, status, sample) = unsafe { windows_mft::process_output(&self.transform, provided) };
            self.pumps += 1;
            if self.pumps <= 40 || hr.0 != MF_E_TRANSFORM_NEED_MORE_INPUT {
                dbg::log(|| format!("h264dec: ProcessOutput #{} {} status 0x{status:x}", self.pumps, hex_hr(hr.0)));
            }
            if hr.0 == MF_E_TRANSFORM_NEED_MORE_INPUT {
                break;
            }
            if hr.0 == MF_E_TRANSFORM_STREAM_CHANGE || hr.0 == MF_E_TRANSFORM_TYPE_NOT_SET || hr.0 == MF_E_BUFFERTOOSMALL {
                renegotiations += 1;
                if renegotiations > MAX_RENEGOTIATIONS {
                    return Err(VideoFileError::with_code("IMFTransform::ProcessOutput (type never settles)", hr.0));
                }
                self.negotiate_output_type()?;
                continue;
            }
            if hr.is_err() {
                return Err(VideoFileError::with_code("IMFTransform::ProcessOutput", hr.0));
            }
            let Some(sample) = sample else { break };
            if self.width == 0 || self.height == 0 {
                // A decoder that emitted a picture without ever reporting a
                // stream change still carries the size on its current type.
                if let Ok(current) = unsafe { windows_mft::get_output_current_type(&self.transform) } {
                    unsafe { self.refresh_output_geometry(&current)? };
                }
                if self.width == 0 || self.height == 0 {
                    dbg::log(|| "h264dec: decoded sample but no frame size known, dropping".to_string());
                    self.pending_pts.pop_front();
                    continue;
                }
            }
            let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(|e| hr_err("ConvertToContiguousBuffer", e))?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }.map_err(|e| hr_err("Lock(out)", e))?;
            let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
            let width = self.width as usize;
            let height = self.height as usize;
            let uv_height = height.div_ceil(2);
            let stride = self.output_stride.max(width);
            let mut nv12 = vec![0u8; width * (height + uv_height)];
            if stride * (height + uv_height) <= bytes.len() {
                for row in 0..height {
                    nv12[row * width..(row + 1) * width]
                        .copy_from_slice(&bytes[row * stride..row * stride + width]);
                }
                for row in 0..uv_height {
                    let src = (height + row) * stride;
                    let dst = (height + row) * width;
                    nv12[dst..dst + width].copy_from_slice(&bytes[src..src + width]);
                }
            } else {
                let copy_len = nv12.len().min(bytes.len());
                nv12[..copy_len].copy_from_slice(&bytes[..copy_len]);
            }
            unsafe { buffer.Unlock() }.map_err(|e| hr_err("Unlock(out)", e))?;
            let pts_100ns = self.pending_pts.pop_front().unwrap_or(0);
            self.frames_out += 1;
            dbg::log(|| {
                format!(
                    "h264dec: frame #{} {}x{} ({} bytes locked, stride {}) pts {}",
                    self.frames_out, width, height, len, stride, pts_100ns
                )
            });
            frames.push(DecodedFrame { width: self.width, height: self.height, nv12, pts_100ns });
        }
        Ok(frames)
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        self.drain_available()
    }
}

// SAFETY: see `WindowsStreamEncoder`'s identical `Send` impl in
// windows_stream_encoder.rs — same reasoning (Mutex-enforced exclusive
// access + per-thread MTA re-assertion on every entry point).
unsafe impl Send for WindowsStreamDecoder {}
