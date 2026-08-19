//! Windows backend for the low-latency stream encoder: the Microsoft
//! software H.264 encoder MFT (`CLSID_CMSH264EncoderMFT`), driven directly
//! via `IMFTransform::ProcessInput`/`ProcessOutput` (see [`crate::windows_
//! mft`]) instead of the file-based `IMFSinkWriter` seam `windows_encoder.rs`
//! uses.
//!
//! **UNVERIFIED — cross-compiled only, never run.** There is no Windows
//! machine in this agent's environment; this file has only ever been
//! `cargo check -p makepad-video --target x86_64-pc-windows-msvc`-checked
//! (type/signature validation only — it cannot catch COM/HRESULT semantic
//! bugs). Treat it as a first draft that needs real Windows testing.
//!
//! Design choices forced by the vendored `windows-rs` binding subset (see
//! [`crate::windows_mft`]'s module doc for why):
//! - **No hardware-transform enumeration.** `MFTEnumEx`/`IMFActivate`/
//!   `MFT_REGISTER_TYPE_INFO` are not in the vendored bindings and were not
//!   hand-added (out of scope for what could be verified here). This always
//!   instantiates the Microsoft software H.264 encoder MFT via
//!   `CoCreateInstance` with its well-known, publicly documented CLSID —
//!   always present since Windows 7, but never hardware-accelerated.
//! - **No `ICodecAPI`** (also not bound; hand-rolling its COM vtable was
//!   judged too much unverifiable risk for this pass) — so there is no
//!   `CODECAPI_AVEncCommonRateControlMode`/B-frame-count/GOP-size tuning
//!   knob and no `CODECAPI_AVEncVideoForceKeyFrame`. Instead:
//!   - Low latency is requested via the well-known `MF_LOW_LATENCY` MFT
//!     attribute (widely honored by Microsoft's own MFTs).
//!   - Bitrate is requested via `MF_MT_AVG_BITRATE` on the output type (the
//!     same knob the file-based sink-writer encoder uses).
//!   - `request_keyframe()` AND periodic `keyint` enforcement are both
//!     implemented by tearing down and recreating the encoder MFT — a fresh
//!     encoder session always starts with an IDR. Heavier than `ICodecAPI::
//!     SetValue(CODECAPI_AVEncVideoForceKeyFrame, ...)` but needs nothing
//!     beyond what's already bound, and is a real keyframe either way.
//! - Output byte format: documented as Annex-B for this MFT by Microsoft,
//!   but since that could not be verified here, [`normalize_to_annex_b`]
//!   defensively detects a start code and falls back to converting from
//!   AVCC (4-byte length-prefixed) if one isn't present.

use crate::annex_b;
use crate::stream_encoder::{EncodedPacket, StreamVideoCodec, VideoStreamEncoderOptions};
use crate::windows_encoder::{ensure_media_foundation, hr_err};
use crate::windows_mft::{
    self, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MF_E_TRANSFORM_NEED_MORE_INPUT,
};
use crate::VideoFileError;
use std::collections::VecDeque;
use windows::{
    core::GUID,
    Win32::Media::MediaFoundation::{
        IMFSample, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video,
        MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive, MF_MT_AVG_BITRATE,
        MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
        MF_MT_SUBTYPE,
    },
    Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
};

/// Well-known Microsoft software H.264 encoder MFT CLSID (mfh264enc.dll) —
/// stable and documented since Windows 7; not in the vendored bindings (see
/// module doc).
const CLSID_CMS_H264_ENCODER_MFT: GUID = GUID::from_u128(0x6ca50344_051a_4ded_9779_a43305165e35);

/// `MF_LOW_LATENCY` attribute GUID (mfapi.h) — not in the vendored bindings.
const MF_LOW_LATENCY: GUID = GUID::from_u128(0x9c27891a_ed7a_40e1_88e8_b22727a24f00);

fn create_and_negotiate(options: &VideoStreamEncoderOptions) -> Result<(IMFTransform, bool, u32), VideoFileError> {
    if !matches!(options.codec, StreamVideoCodec::H264) {
        return Err(VideoFileError::new("windows stream encoder: only H264 is implemented"));
    }
    let transform: IMFTransform = unsafe { CoCreateInstance(&CLSID_CMS_H264_ENCODER_MFT, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| hr_err("CoCreateInstance(CLSID_CMSH264EncoderMFT)", e))?;

    unsafe {
        if let Ok(attributes) = transform.GetAttributes() {
            let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
        }

        let out_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(out)", e))?;
        out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| hr_err("set out major type", e))?;
        out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| hr_err("set out subtype", e))?;
        out_type
            .SetUINT32(&MF_MT_AVG_BITRATE, options.bitrate_kbps.saturating_mul(1000))
            .map_err(|e| hr_err("set out bitrate", e))?;
        out_type
            .SetUINT64(&MF_MT_FRAME_SIZE, ((options.width as u64) << 32) | options.height as u64)
            .map_err(|e| hr_err("set out frame size", e))?;
        out_type
            .SetUINT64(&MF_MT_FRAME_RATE, ((options.fps as u64) << 32) | 1u64)
            .map_err(|e| hr_err("set out frame rate", e))?;
        out_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| hr_err("set out interlace mode", e))?;
        windows_mft::set_output_type(&transform, &out_type).map_err(|e| hr_err("IMFTransform::SetOutputType", e))?;

        let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(in)", e))?;
        in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| hr_err("set in major type", e))?;
        in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| hr_err("set in subtype", e))?;
        in_type
            .SetUINT64(&MF_MT_FRAME_SIZE, ((options.width as u64) << 32) | options.height as u64)
            .map_err(|e| hr_err("set in frame size", e))?;
        in_type
            .SetUINT64(&MF_MT_FRAME_RATE, ((options.fps as u64) << 32) | 1u64)
            .map_err(|e| hr_err("set in frame rate", e))?;
        in_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| hr_err("set in interlace mode", e))?;
        in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, options.width).map_err(|e| hr_err("set in stride", e))?;
        windows_mft::set_input_type(&transform, &in_type).map_err(|e| hr_err("IMFTransform::SetInputType", e))?;

        let stream_info =
            windows_mft::get_output_stream_info(&transform).map_err(|e| hr_err("IMFTransform::GetOutputStreamInfo", e))?;
        let provides_samples = stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES != 0;

        windows_mft::process_message(&transform, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(|e| hr_err("ProcessMessage(BEGIN_STREAMING)", e))?;
        windows_mft::process_message(&transform, MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(|e| hr_err("ProcessMessage(START_OF_STREAM)", e))?;

        Ok((transform, provides_samples, stream_info.cbSize))
    }
}

fn make_input_sample(nv12: &[u8], pts_100ns: i64, duration_100ns: i64) -> Result<IMFSample, VideoFileError> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(nv12.len() as u32).map_err(|e| hr_err("MFCreateMemoryBuffer", e))?;
        let mut ptr = std::ptr::null_mut();
        buffer.Lock(&mut ptr, None, None).map_err(|e| hr_err("IMFMediaBuffer::Lock", e))?;
        std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
        buffer.Unlock().map_err(|e| hr_err("IMFMediaBuffer::Unlock", e))?;
        buffer.SetCurrentLength(nv12.len() as u32).map_err(|e| hr_err("SetCurrentLength", e))?;
        let sample = MFCreateSample().map_err(|e| hr_err("MFCreateSample", e))?;
        sample.AddBuffer(&buffer).map_err(|e| hr_err("AddBuffer", e))?;
        sample.SetSampleTime(pts_100ns).map_err(|e| hr_err("SetSampleTime", e))?;
        sample.SetSampleDuration(duration_100ns).map_err(|e| hr_err("SetSampleDuration", e))?;
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

/// Detects an existing Annex-B start code; converts from AVCC (4-byte
/// length-prefixed) otherwise. See the module doc: this MFT's output format
/// is documented as Annex-B, but that could not be verified here.
fn normalize_to_annex_b(bytes: &[u8]) -> Vec<u8> {
    let looks_like_annex_b =
        bytes.len() >= 3 && (bytes[0..3] == [0, 0, 1] || (bytes.len() >= 4 && bytes[0..4] == [0, 0, 0, 1]));
    if looks_like_annex_b {
        bytes.to_vec()
    } else {
        annex_b::avcc_to_annex_b(bytes)
    }
}

pub struct WindowsStreamEncoder {
    options: VideoStreamEncoderOptions,
    transform: IMFTransform,
    provides_samples: bool,
    output_buffer_size: u32,
    duration_100ns: i64,
    frames_since_keyframe: u32,
    force_keyframe: bool,
    pending_pts: VecDeque<i64>,
}

impl WindowsStreamEncoder {
    pub fn new(options: &VideoStreamEncoderOptions) -> Result<Self, VideoFileError> {
        ensure_media_foundation()?;
        let (transform, provides_samples, output_buffer_size) = create_and_negotiate(options)?;
        Ok(Self {
            options: *options,
            transform,
            provides_samples,
            output_buffer_size,
            duration_100ns: 10_000_000 / options.fps.max(1) as i64,
            frames_since_keyframe: 0,
            force_keyframe: true, // the first frame is always a keyframe
            pending_pts: VecDeque::new(),
        })
    }

    fn recreate(&mut self) -> Result<(), VideoFileError> {
        let (transform, provides_samples, output_buffer_size) = create_and_negotiate(&self.options)?;
        self.transform = transform;
        self.provides_samples = provides_samples;
        self.output_buffer_size = output_buffer_size;
        self.frames_since_keyframe = 0;
        self.pending_pts.clear();
        Ok(())
    }

    pub fn push_frame_nv12(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        // COM apartment state is thread-local: `new()` initialized the MTA
        // only on the thread that constructed this encoder. Since this type
        // is `Send` (see the impl below) and callers may legitimately push
        // frames from a different thread than the one that created it (e.g.
        // `makepad-asset-ai`'s realtime session, whose worker thread is not
        // necessarily the HTTP thread), re-assert MTA membership on whatever
        // thread is calling now — idempotent and cheap (a thread already in
        // the MTA is a no-op; `MFStartup` itself only runs once, process-wide).
        ensure_media_foundation()?;
        let keyint = self.options.keyint.max(1);
        if self.force_keyframe || self.frames_since_keyframe >= keyint {
            self.recreate()?;
        }
        self.force_keyframe = false;
        self.frames_since_keyframe += 1;

        let sample = make_input_sample(nv12, pts_100ns, self.duration_100ns)?;
        let mut packets = Vec::new();
        let hr = unsafe { windows_mft::process_input(&self.transform, &sample) };
        if hr.is_err() {
            // Generic "drain first, then retry once" — the exact rejection
            // HRESULT (MF_E_NOTACCEPTING) is not in the vendored bindings,
            // so this does not match it specifically; any ProcessInput
            // failure is treated as "the MFT wants its output drained
            // first."
            packets.append(&mut self.drain_available()?);
            let retry_hr = unsafe { windows_mft::process_input(&self.transform, &sample) };
            if retry_hr.is_err() {
                return Err(VideoFileError::with_code("IMFTransform::ProcessInput", retry_hr.0));
            }
        }
        self.pending_pts.push_back(pts_100ns);
        packets.append(&mut self.drain_available()?);
        Ok(packets)
    }

    fn drain_available(&mut self) -> Result<Vec<EncodedPacket>, VideoFileError> {
        let mut packets = Vec::new();
        loop {
            let provided = if self.provides_samples {
                None
            } else {
                Some(make_output_sample(self.output_buffer_size)?)
            };
            let (hr, sample) = unsafe { windows_mft::process_output(&self.transform, provided) };
            if hr.0 == MF_E_TRANSFORM_NEED_MORE_INPUT {
                break;
            }
            if hr.is_err() {
                return Err(VideoFileError::with_code("IMFTransform::ProcessOutput", hr.0));
            }
            let Some(sample) = sample else { break };
            let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(|e| hr_err("ConvertToContiguousBuffer", e))?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }.map_err(|e| hr_err("Lock(out)", e))?;
            let raw = unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec();
            unsafe { buffer.Unlock() }.map_err(|e| hr_err("Unlock(out)", e))?;

            let annex_b_data = normalize_to_annex_b(&raw);
            let is_key = annex_b::split_annex_b(&annex_b_data)
                .iter()
                .any(|nal| annex_b::nal_unit_type(nal) == annex_b::NAL_TYPE_SPS);
            let pts_100ns = self.pending_pts.pop_front().unwrap_or(0);
            packets.push(EncodedPacket { data: annex_b_data, pts_100ns, is_key });
        }
        Ok(packets)
    }

    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }
}

// SAFETY (UNVERIFIED — no Windows machine to confirm on): `IMFTransform`
// wraps a raw COM pointer (`NonNull<c_void>`, `!Send` by default). This
// encoder is only ever accessed through a `Mutex` (exclusive access
// enforced), which rules out the memory-safety hazard `Send` is about.
// The remaining question is COM-apartment correctness: `push_frame_nv12`
// re-asserts MTA membership (`ensure_media_foundation()`) on whatever
// thread calls it, and MTA-created in-proc objects are documented to
// tolerate being called from any MTA thread — so moving ownership between
// threads (not concurrent access) should be sound as long as every calling
// thread is in the MTA, which the re-assert guarantees. Flagged as
// unverified because this is exactly the kind of thing that only a real
// Windows COM run can actually confirm.
unsafe impl Send for WindowsStreamEncoder {}
