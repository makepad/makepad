//! Windows backend for the low-latency stream encoder: the Microsoft
//! software H.264 encoder MFT (`CLSID_CMSH264EncoderMFT`), driven directly
//! via `IMFTransform::ProcessInput`/`ProcessOutput` (see [`crate::windows_
//! mft`]) instead of the file-based `IMFSinkWriter` seam `windows_encoder.rs`
//! uses.
//!
//! Uses the Microsoft software H.264 MFT. Keyframe requests recreate its
//! session; bitrate and low latency are configured through media attributes.
//! Output is normalized to Annex-B for the transport.

use crate::annex_b;
use crate::stream_encoder::{EncodedPacket, StreamVideoCodec, VideoStreamEncoderOptions};
use crate::windows_encoder::{ensure_media_foundation, hr_err};
use crate::windows_mft;
use crate::VideoFileError;
use std::collections::VecDeque;
use windows::Win32::Media::MediaFoundation::*;
use windows::{
    core::HRESULT,
    Win32::Media::MediaFoundation::{
        IMFSample, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
        MFMediaType_Video, MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
        MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
        MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    },
    Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
};

fn create_and_negotiate(
    options: &VideoStreamEncoderOptions,
) -> Result<(IMFTransform, bool, u32), VideoFileError> {
    if !matches!(options.codec, StreamVideoCodec::H264) {
        return Err(VideoFileError::new(
            "windows stream encoder: only H264 is implemented",
        ));
    }
    let transform: IMFTransform = unsafe {
        CoCreateInstance(
            &CMSH264EncoderMFT,
            None,
            CLSCTX_INPROC_SERVER,
            &IMFTransform::IID,
        )
        .and_then(|raw| IMFTransform::from_raw(raw))
    }
    .map_err(|e| hr_err("CoCreateInstance(CLSID_CMSH264EncoderMFT)", e))?;

    unsafe {
        if let Ok(attributes) = transform.GetAttributes() {
            let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
        }

        let out_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(out)", e))?;
        out_type
            .as_IMFAttributes()
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| hr_err("set out major type", e))?;
        out_type
            .as_IMFAttributes()
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|e| hr_err("set out subtype", e))?;
        out_type
            .as_IMFAttributes()
            .SetUINT32(
                &MF_MT_AVG_BITRATE,
                options.bitrate_kbps.saturating_mul(1000),
            )
            .map_err(|e| hr_err("set out bitrate", e))?;
        out_type
            .as_IMFAttributes()
            .SetUINT64(
                &MF_MT_FRAME_SIZE,
                ((options.width as u64) << 32) | options.height as u64,
            )
            .map_err(|e| hr_err("set out frame size", e))?;
        out_type
            .as_IMFAttributes()
            .SetUINT64(&MF_MT_FRAME_RATE, ((options.fps as u64) << 32) | 1u64)
            .map_err(|e| hr_err("set out frame rate", e))?;
        out_type
            .as_IMFAttributes()
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| hr_err("set out interlace mode", e))?;
        transform
            .SetOutputType(0, &out_type, 0)
            .map_err(|e| hr_err("IMFTransform::SetOutputType", e))?;

        let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(in)", e))?;
        in_type
            .as_IMFAttributes()
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| hr_err("set in major type", e))?;
        in_type
            .as_IMFAttributes()
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| hr_err("set in subtype", e))?;
        in_type
            .as_IMFAttributes()
            .SetUINT64(
                &MF_MT_FRAME_SIZE,
                ((options.width as u64) << 32) | options.height as u64,
            )
            .map_err(|e| hr_err("set in frame size", e))?;
        in_type
            .as_IMFAttributes()
            .SetUINT64(&MF_MT_FRAME_RATE, ((options.fps as u64) << 32) | 1u64)
            .map_err(|e| hr_err("set in frame rate", e))?;
        in_type
            .as_IMFAttributes()
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| hr_err("set in interlace mode", e))?;
        in_type
            .as_IMFAttributes()
            .SetUINT32(&MF_MT_DEFAULT_STRIDE, options.width)
            .map_err(|e| hr_err("set in stride", e))?;
        transform
            .SetInputType(0, &in_type, 0)
            .map_err(|e| hr_err("IMFTransform::SetInputType", e))?;

        let stream_info = transform
            .GetOutputStreamInfo(0)
            .map_err(|e| hr_err("IMFTransform::GetOutputStreamInfo", e))?;
        let provides_samples =
            stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;

        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(|e| hr_err("ProcessMessage(BEGIN_STREAMING)", e))?;
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(|e| hr_err("ProcessMessage(START_OF_STREAM)", e))?;

        Ok((transform, provides_samples, stream_info.cbSize))
    }
}

fn make_input_sample(
    nv12: &[u8],
    pts_100ns: i64,
    duration_100ns: i64,
) -> Result<IMFSample, VideoFileError> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(nv12.len() as u32)
            .map_err(|e| hr_err("MFCreateMemoryBuffer", e))?;
        let mut ptr = std::ptr::null_mut();
        buffer
            .Lock(&mut ptr, None, None)
            .map_err(|e| hr_err("IMFMediaBuffer::Lock", e))?;
        std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
        buffer
            .Unlock()
            .map_err(|e| hr_err("IMFMediaBuffer::Unlock", e))?;
        buffer
            .SetCurrentLength(nv12.len() as u32)
            .map_err(|e| hr_err("SetCurrentLength", e))?;
        let sample = MFCreateSample().map_err(|e| hr_err("MFCreateSample", e))?;
        sample
            .AddBuffer(&buffer)
            .map_err(|e| hr_err("AddBuffer", e))?;
        sample
            .SetSampleTime(pts_100ns)
            .map_err(|e| hr_err("SetSampleTime", e))?;
        sample
            .SetSampleDuration(duration_100ns)
            .map_err(|e| hr_err("SetSampleDuration", e))?;
        Ok(sample)
    }
}

fn make_output_sample(size: u32) -> Result<IMFSample, VideoFileError> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(size.max(1))
            .map_err(|e| hr_err("MFCreateMemoryBuffer(out)", e))?;
        let sample = MFCreateSample().map_err(|e| hr_err("MFCreateSample(out)", e))?;
        sample
            .AddBuffer(&buffer)
            .map_err(|e| hr_err("AddBuffer(out)", e))?;
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
        let hr = unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .err()
                .unwrap_or(HRESULT(0))
        };
        if hr.is_err() {
            // Preserve the pump's drain-before-retry policy. The retry's
            // HRESULT is reported if the transform still rejects the input.
            packets.append(&mut self.drain_available()?);
            let retry_hr = unsafe {
                self.transform
                    .ProcessInput(0, &sample, 0)
                    .err()
                    .unwrap_or(HRESULT(0))
            };
            if retry_hr.is_err() {
                return Err(VideoFileError::with_code(
                    "IMFTransform::ProcessInput",
                    retry_hr.0,
                ));
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
            let (hr, _status, sample) =
                unsafe { windows_mft::process_output(&self.transform, provided) };
            if hr.0 == MF_E_TRANSFORM_NEED_MORE_INPUT.0 {
                break;
            }
            if hr.is_err() {
                return Err(VideoFileError::with_code(
                    "IMFTransform::ProcessOutput",
                    hr.0,
                ));
            }
            let Some(sample) = sample else { break };
            let buffer = unsafe { sample.ConvertToContiguousBuffer() }
                .map_err(|e| hr_err("ConvertToContiguousBuffer", e))?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }
                .map_err(|e| hr_err("Lock(out)", e))?;
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

// SAFETY: the encoder is accessed exclusively. Each call re-establishes MTA
// membership in ensure_media_foundation before using the MTA-created transform.
unsafe impl Send for WindowsStreamEncoder {}
