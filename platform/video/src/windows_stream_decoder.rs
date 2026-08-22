//! Windows backend for the low-latency stream decoder: the Microsoft
//! H.264 decoder MFT (`CLSID_CMSH264DecoderMFT`), driven directly via
//! `IMFTransform::ProcessInput`/`ProcessOutput` (see [`crate::windows_
//! mft`]).
//!
//! **UNVERIFIED — cross-compiled only, never run.** See `windows_stream_
//! encoder`'s module doc for the full explanation (no Windows machine
//! available; several constants below are hand-derived because the vendored
//! `windows-rs` binding subset in this repo does not include them).
//!
//! No DXVA/hardware surface path (the user-facing spec calls this
//! "optional, later") — output is read back to system memory NV12 via
//! `IMFMediaBuffer::Lock`, same as the file-based decoder.
//!
//! The input type is set to bare H.264 (no frame-size hint — the decoder
//! determines the real picture size from the bitstream's SPS). The first
//! `ProcessOutput` calls are expected to return `MF_E_TRANSFORM_STREAM_
//! CHANGE` once the decoder has parsed enough of the stream to know its
//! output format; `renegotiate_output_type` handles that by walking `Get
//! OutputAvailableType` for an NV12 entry and calling `SetOutputType`.

use crate::stream_decoder::DecodedFrame;
use crate::stream_encoder::StreamVideoCodec;
use crate::windows_encoder::{ensure_media_foundation, hr_err};
use crate::windows_mft::{
    self, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
};
use crate::VideoFileError;
use std::collections::VecDeque;
use windows::{
    core::GUID,
    Win32::Media::MediaFoundation::{
        IMFSample, IMFTransform, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video,
        MFVideoFormat_H264, MFVideoFormat_NV12, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    },
    Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
};

/// Well-known Microsoft H.264 decoder MFT CLSID (msmpeg2vdec.dll) — stable
/// and documented since Windows 7; not in the vendored bindings.
const CLSID_CMS_H264_DECODER_MFT: GUID = GUID::from_u128(0x62ce7e72_4c71_4d20_b15d_452831a87d9d);

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

pub struct WindowsStreamDecoder {
    transform: IMFTransform,
    output_negotiated: bool,
    provides_samples: bool,
    output_buffer_size: u32,
    width: u32,
    height: u32,
    pending_pts: VecDeque<i64>,
}

impl WindowsStreamDecoder {
    pub fn new(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        if !matches!(codec, StreamVideoCodec::H264) {
            return Err(VideoFileError::new("windows stream decoder: only H264 is implemented"));
        }
        let transform = create_transform()?;
        unsafe {
            let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(in)", e))?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| hr_err("set in major type", e))?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| hr_err("set in subtype", e))?;
            windows_mft::set_input_type(&transform, &in_type).map_err(|e| hr_err("IMFTransform::SetInputType", e))?;
            windows_mft::process_message(&transform, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| hr_err("ProcessMessage(BEGIN_STREAMING)", e))?;
            windows_mft::process_message(&transform, MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| hr_err("ProcessMessage(START_OF_STREAM)", e))?;
        }
        Ok(Self {
            transform,
            output_negotiated: false,
            provides_samples: false,
            output_buffer_size: 0,
            width: 0,
            height: 0,
            pending_pts: VecDeque::new(),
        })
    }

    /// Walks `GetOutputAvailableType` looking for an NV12 entry (the decoder
    /// only offers real entries once it has parsed enough of the bitstream —
    /// this is called in response to `MF_E_TRANSFORM_STREAM_CHANGE`) and
    /// commits it with `SetOutputType`.
    fn renegotiate_output_type(&mut self) -> Result<(), VideoFileError> {
        unsafe {
            let mut chosen = None;
            for index in 0..16u32 {
                let Ok(candidate) = windows_mft::get_output_available_type(&self.transform, index) else {
                    break;
                };
                if let Ok(subtype) = candidate.GetGUID(&MF_MT_SUBTYPE) {
                    if subtype == MFVideoFormat_NV12 {
                        chosen = Some(candidate);
                        break;
                    }
                }
            }
            let Some(chosen_type) = chosen else {
                return Err(VideoFileError::new(
                    "windows stream decoder: no NV12 output type offered by CMSH264DecoderMFT",
                ));
            };
            let packed = chosen_type.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| hr_err("GetUINT64(FRAME_SIZE)", e))?;
            self.width = (packed >> 32) as u32;
            self.height = (packed & 0xFFFF_FFFF) as u32;
            windows_mft::set_output_type(&self.transform, &chosen_type).map_err(|e| hr_err("SetOutputType(NV12)", e))?;
            let stream_info =
                windows_mft::get_output_stream_info(&self.transform).map_err(|e| hr_err("GetOutputStreamInfo", e))?;
            self.provides_samples = stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES != 0;
            self.output_buffer_size = stream_info.cbSize;
            self.output_negotiated = true;
        }
        Ok(())
    }

    pub fn push_packet(&mut self, annex_b_data: &[u8], pts_100ns: i64) -> Result<Vec<DecodedFrame>, VideoFileError> {
        // See `WindowsStreamEncoder::push_frame_nv12`'s identical comment —
        // this type is `Send` and may be called from a different thread
        // than the one that constructed it; re-assert MTA membership here.
        ensure_media_foundation()?;
        let sample = make_input_sample(annex_b_data, pts_100ns)?;
        let hr = unsafe { windows_mft::process_input(&self.transform, &sample) };
        if hr.is_err() {
            return Err(VideoFileError::with_code("IMFTransform::ProcessInput", hr.0));
        }
        self.pending_pts.push_back(pts_100ns);
        self.drain_available()
    }

    fn drain_available(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        let mut frames = Vec::new();
        loop {
            if !self.output_negotiated {
                // Nothing to pull yet — the decoder hasn't told us its
                // output format (needs SPS/PPS + at least one VCL NAL).
                break;
            }
            let provided = if self.provides_samples { None } else { Some(make_output_sample(self.output_buffer_size)?) };
            let (hr, sample) = unsafe { windows_mft::process_output(&self.transform, provided) };
            if hr.0 == MF_E_TRANSFORM_NEED_MORE_INPUT {
                break;
            }
            if hr.0 == MF_E_TRANSFORM_STREAM_CHANGE {
                self.renegotiate_output_type()?;
                continue;
            }
            if hr.is_err() {
                return Err(VideoFileError::with_code("IMFTransform::ProcessOutput", hr.0));
            }
            let Some(sample) = sample else { break };
            let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(|e| hr_err("ConvertToContiguousBuffer", e))?;
            let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
            unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }.map_err(|e| hr_err("Lock(out)", e))?;
            let nv12 = unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec();
            unsafe { buffer.Unlock() }.map_err(|e| hr_err("Unlock(out)", e))?;
            let pts_100ns = self.pending_pts.pop_front().unwrap_or(0);
            frames.push(DecodedFrame { width: self.width, height: self.height, nv12, pts_100ns });
        }
        Ok(frames)
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        self.drain_available()
    }
}

// SAFETY (UNVERIFIED): see `WindowsStreamEncoder`'s identical `Send` impl
// in windows_stream_encoder.rs — same reasoning (Mutex-enforced exclusive
// access + per-thread MTA re-assertion on every entry point).
unsafe impl Send for WindowsStreamDecoder {}
