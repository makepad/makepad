//! Windows backend for the low-latency stream encoder.
//!
//! - The GPU's own H.264 encoder MFT (`MFTEnumEx` with the hardware flag: NVIDIA's, AMD's or
//!   Intel's), asynchronous, on the D3D11 device of the HAL's shared textures
//!   (`makepad_hal::shared_texture`). Pictures go in as NV12 DXGI surfaces: a shared texture's
//!   picture is converted on the GPU (one video-processor blit), CPU NV12 is uploaded once.
//!   Key frames on request through `CODECAPI_AVEncVideoForceKeyFrame`, low-delay VBR, no B
//!   frames, an open-ended GOP.
//! - Without a hardware encoder: the Microsoft software H.264 MFT
//!   (`CLSID_CMSH264EncoderMFT`) on CPU NV12, driven synchronously; a key frame request
//!   recreates its session.
//!
//! Output is normalized to Annex-B for the transport.

use crate::annex_b;
use crate::stream_encoder::{EncodeTimings, EncodedPacket, EncoderCaps, StreamEncoderOs, StreamTile, StreamTuning, StreamVideoCodec, VideoStreamEncoderOptions};
use crate::surface::{GpuCompletion, SurfaceFormat, SurfaceHandle, SurfaceKind};
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

/// The GPU's H.264 encoder MFT (asynchronous: driven by its NeedInput / HaveOutput events).
struct HardwareMft {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    _manager: IMFDXGIDeviceManager,
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    /// NeedInput events not answered yet.
    need_input: u32,
    provides_samples: bool,
    out_size: u32,
    size: [u32; 2],
    duration_100ns: i64,
    name: String,
    /// The stream's newest SPS and PPS (Annex-B): sent in band ahead of every IDR the encoder
    /// gives without them (a forced key frame of some MFTs carries only the IDR slice).
    headers: Vec<u8>,
    /// H.265 (else H.264): which NAL types are parameter sets and IDRs.
    hevc: bool,
}

/// The MFT output subtype of `codec`.
fn subtype(codec: StreamVideoCodec) -> windows::core::GUID {
    match codec {
        StreamVideoCodec::H264 => MFVideoFormat_H264,
        StreamVideoCodec::Hevc => MFVideoFormat_HEVC,
    }
}

/// Whether this machine has a hardware H.265 encoder MFT that takes NV12 (asked once).
fn hardware_hevc() -> bool {
    static HEVC: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HEVC.get_or_init(|| unsafe {
        if ensure_media_foundation().is_err() {
            return false;
        }
        let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_HEVC };
        let mut activates: *mut *mut std::ffi::c_void = std::ptr::null_mut();
        let mut count = 0u32;
        let ok = MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0), Some(&input), Some(&output), &mut activates, &mut count).is_ok();
        if !activates.is_null() {
            for i in 0..count as usize {
                drop(IMFActivate::from_raw(*activates.add(i)));
            }
            windows::Win32::System::Com::CoTaskMemFree(Some(activates as *const _));
        }
        ok && count > 0
    })
}

impl HardwareMft {
    /// The first hardware encoder of the stream's codec that takes NV12, set up for a live view, or None
    /// (no hardware encoder, or it refused the setup: `Err` says why).
    unsafe fn open(options: &VideoStreamEncoderOptions) -> Result<Option<HardwareMft>, VideoFileError> {
        let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype(options.codec) };
        let mut activates: *mut *mut std::ffi::c_void = std::ptr::null_mut();
        let mut count = 0u32;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
            Some(&input),
            Some(&output),
            &mut activates,
            &mut count,
        )
        .map_err(|e| hr_err("MFTEnumEx", e))?;
        if count == 0 || activates.is_null() {
            return Ok(None);
        }
        let list: Vec<IMFActivate> = (0..count as usize).filter_map(|i| IMFActivate::from_raw(*activates.add(i)).ok()).collect();
        windows::Win32::System::Com::CoTaskMemFree(Some(activates as *const _));
        // the first that works with the device of the HAL's shared textures: an encoder of
        // another GPU (an iGPU next to the discrete one) refuses that device
        let mut refused = Vec::new();
        for activate in &list {
            let name = read_friendly_name(activate.as_IMFAttributes()).unwrap_or_else(|| "hardware H.264 encoder".into());
            match Self::open_one(activate, name.clone(), options) {
                Ok(hw) => return Ok(Some(hw)),
                Err(e) => {
                    let _ = activate.ShutdownObject();
                    refused.push(format!("{name}: {e:?}"));
                }
            }
        }
        Err(VideoFileError::new(refused.join("; ")))
    }

    unsafe fn open_one(activate: &IMFActivate, name: String, options: &VideoStreamEncoderOptions) -> Result<HardwareMft, VideoFileError> {
        use windows::Win32::Graphics::Direct3D11::ID3D11Multithread;
        let transform = IMFTransform::from_raw(activate.ActivateObject(&IMFTransform::IID).map_err(|e| hr_err("IMFActivate::ActivateObject", e))?)
            .map_err(|e| hr_err("IMFTransform", e))?;
        let attributes = transform.GetAttributes().map_err(|e| hr_err("IMFTransform::GetAttributes", e))?;
        // an asynchronous MFT works only once unlocked
        if attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0 {
            attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).map_err(|e| hr_err("MF_TRANSFORM_ASYNC_UNLOCK", e))?;
        }
        let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
        // the device of the HAL's shared textures: pictures stay where they were rendered
        let device = makepad_hal::shared_texture::device().map_err(VideoFileError::new)?;
        if let Ok(mt) = ID3D11Multithread::query(device.as_raw()) {
            let _ = mt.SetMultithreadProtected(true);
        }
        let mut token = 0u32;
        let mut manager: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager).map_err(|e| hr_err("MFCreateDXGIDeviceManager", e))?;
        let manager = manager.ok_or_else(|| VideoFileError::new("MFCreateDXGIDeviceManager returned nothing"))?;
        manager.ResetDevice(device.as_IUnknown(), token).map_err(|e| hr_err("IMFDXGIDeviceManager::ResetDevice", e))?;
        transform
            .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
            .map_err(|e| hr_err("ProcessMessage(SET_D3D_MANAGER)", e))?;
        // a live view: low-delay rate control, no B frames, a key frame only on request
        let mut notes = Vec::new();
        let mut codec = |key: &windows::core::GUID, value: u32, what: &str| {
            if let Err(e) = windows_mft::set_codec_api_u32(&transform, key, value) {
                notes.push(format!("{what} not taken ({:#x})", e.0));
            }
        };
        codec(&CODECAPI_AVLowLatencyMode, 1, "low latency mode");
        codec(&CODECAPI_AVEncCommonRateControlMode, eAVEncCommonRateControlMode_LowDelayVBR.0 as u32, "low-delay VBR");
        codec(&CODECAPI_AVEncCommonMeanBitRate, options.bitrate_kbps.saturating_mul(1000), "mean bitrate");
        // a peak of twice the mean: a desktop's changes come in bursts (a scroll, a window)
        codec(&CODECAPI_AVENC_COMMON_MAX_BITRATE, options.bitrate_kbps.saturating_mul(2000), "max bitrate");
        codec(&CODECAPI_AVEncMPVDefaultBPictureCount, 0, "no B frames");
        if options.keyint > 0 {
            codec(&CODECAPI_AVEncMPVGOPSize, options.keyint, "GOP size");
        }
        set_types(&transform, options)?;
        let stream_info = transform.GetOutputStreamInfo(0).map_err(|e| hr_err("IMFTransform::GetOutputStreamInfo", e))?;
        let provides_samples = stream_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(|e| hr_err("ProcessMessage(BEGIN_STREAMING)", e))?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(|e| hr_err("ProcessMessage(START_OF_STREAM)", e))?;
        let events = IMFMediaEventGenerator::query(transform.as_raw()).map_err(|e| hr_err("IMFMediaEventGenerator", e))?;
        Ok(HardwareMft {
            transform,
            events,
            _manager: manager,
            device,
            need_input: 0,
            provides_samples,
            out_size: stream_info.cbSize,
            size: [options.width, options.height],
            duration_100ns: 10_000_000 / options.fps.max(1) as i64,
            name: if notes.is_empty() { name } else { format!("{name}; {}", notes.join(", ")) },
            headers: Vec::new(),
            hevc: options.codec == StreamVideoCodec::Hevc,
        })
    }

    /// Encodes the NV12 texture `nv12` (on [`HardwareMft::device`]): waits for the MFT to
    /// want input, gives it the surface, and takes the output it produces for it (none when
    /// the encoder holds the picture back: it comes with a later one).
    unsafe fn encode(&mut self, nv12: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, key: bool, pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
        let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, nv12.as_IUnknown(), 0, false).map_err(|e| hr_err("MFCreateDXGISurfaceBuffer", e))?;
        let sample = MFCreateSample().map_err(|e| hr_err("MFCreateSample", e))?;
        sample.AddBuffer(&buffer).map_err(|e| hr_err("AddBuffer", e))?;
        sample.SetSampleTime(pts_100ns).map_err(|e| hr_err("SetSampleTime", e))?;
        sample.SetSampleDuration(self.duration_100ns).map_err(|e| hr_err("SetSampleDuration", e))?;
        let mut packets = Vec::new();
        while self.need_input == 0 {
            self.next_event(&mut packets)?;
        }
        if key {
            if let Err(e) = windows_mft::set_codec_api_u32(&self.transform, &CODECAPI_AVEncVideoForceKeyFrame, 1) {
                eprintln!("hardware encoder: key frame request not taken ({e:?})");
            }
        }
        self.transform.ProcessInput(0, &sample, 0).map_err(|e| hr_err("IMFTransform::ProcessInput", e))?;
        self.need_input -= 1;
        // its output, or its next request for input (the picture is held back)
        let before = packets.len();
        while packets.len() == before && self.need_input == 0 {
            self.next_event(&mut packets)?;
        }
        for p in &mut packets[before..] {
            p.pts_100ns = pts_100ns;
        }
        Ok(packets)
    }

    /// Waits for the MFT's next event: input wanted, or output ready (taken into `packets`).
    unsafe fn next_event(&mut self, packets: &mut Vec<EncodedPacket>) -> Result<(), VideoFileError> {
        let event = self.events.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)).map_err(|e| hr_err("IMFMediaEventGenerator::GetEvent", e))?;
        let ty = event.GetType().map_err(|e| hr_err("IMFMediaEvent::GetType", e))?;
        if ty == METransformNeedInput.0 as u32 {
            self.need_input += 1;
        } else if ty == METransformHaveOutput.0 as u32 {
            let provided = if self.provides_samples { None } else { Some(make_output_sample(self.out_size)?) };
            let (hr, _status, sample) = windows_mft::process_output(&self.transform, provided);
            if hr.is_err() {
                return Err(VideoFileError::with_code("IMFTransform::ProcessOutput", hr.0));
            }
            if let Some(sample) = sample {
                let buffer = sample.ConvertToContiguousBuffer().map_err(|e| hr_err("ConvertToContiguousBuffer", e))?;
                let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
                buffer.Lock(&mut ptr, None, Some(&mut len)).map_err(|e| hr_err("Lock(out)", e))?;
                let raw = std::slice::from_raw_parts(ptr, len as usize).to_vec();
                buffer.Unlock().map_err(|e| hr_err("Unlock(out)", e))?;
                let (data, is_key) = in_band_headers(&mut self.headers, normalize_to_annex_b(&raw), self.hevc);
                packets.push(EncodedPacket { data, pts_100ns: 0, is_key });
            }
        }
        Ok(())
    }

    /// CPU NV12 into an NV12 texture of the encoder's device.
    unsafe fn upload(&self, nv12: &[u8]) -> Result<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, VideoFileError> {
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.size[0],
            Height: self.size[1],
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        // NV12 initial data: the luma rows, then the chroma rows, one pitch
        let data = D3D11_SUBRESOURCE_DATA { pSysMem: nv12.as_ptr() as _, SysMemPitch: self.size[0], SysMemSlicePitch: 0 };
        let mut out = None;
        self.device.CreateTexture2D(&desc, Some(&data), Some(&mut out)).map_err(|e| hr_err("CreateTexture2D(NV12 upload)", e))?;
        out.ok_or_else(|| VideoFileError::new("CreateTexture2D(NV12 upload) returned nothing"))
    }
}

/// A key frame is an access unit with an IDR slice; each one goes out with the stream's SPS
/// and PPS ahead of it (in band), so a viewer can start or restart its decoder at any IDR.
/// `headers` keeps the newest SPS + PPS the encoder gave.
const NAL_TYPE_AUD: u8 = 9;

/// codecapi.h `CODECAPI_AVEncCommonMaxBitRate` (VT_UI4, bit/s): not in the generated bindings'
/// metadata roots yet (tools/winbind/apis.txt; regenerating needs the .winmd inputs).
const CODECAPI_AVENC_COMMON_MAX_BITRATE: windows::core::GUID = windows::core::GUID { data1: 0x9651eae4, data2: 0x39b9, data3: 0x4ebf, data4: [0x85, 0xef, 0xd7, 0xf4, 0x44, 0xec, 0x74, 0x65] };

fn in_band_headers(headers: &mut Vec<u8>, data: Vec<u8>, hevc: bool) -> (Vec<u8>, bool) {
    let nals = annex_b::split_annex_b(&data);
    let ty = |n: &[u8]| if hevc { annex_b::hevc_nal_unit_type(n) } else { annex_b::nal_unit_type(n) };
    // H.265: VPS, SPS, PPS and the IDR slices (IDR_W_RADL, IDR_N_LP); H.264: SPS, PPS, IDR
    let param = |t: u8| if hevc { matches!(t, annex_b::HEVC_NAL_TYPE_VPS | annex_b::HEVC_NAL_TYPE_SPS | annex_b::HEVC_NAL_TYPE_PPS) } else { matches!(t, annex_b::NAL_TYPE_SPS | annex_b::NAL_TYPE_PPS) };
    let is_idr = |t: u8| if hevc { matches!(t, 19 | 20) } else { t == annex_b::NAL_TYPE_IDR };
    let aud = if hevc { 35 } else { NAL_TYPE_AUD };
    let (sps, pps, idr) = if hevc {
        let has = |t: u8| nals.iter().any(|n| ty(n) == t);
        (has(annex_b::HEVC_NAL_TYPE_VPS) && has(annex_b::HEVC_NAL_TYPE_SPS), has(annex_b::HEVC_NAL_TYPE_PPS), nals.iter().any(|n| is_idr(ty(n))))
    } else {
        let has = |t: u8| nals.iter().any(|n| ty(n) == t);
        (has(annex_b::NAL_TYPE_SPS), has(annex_b::NAL_TYPE_PPS), nals.iter().any(|n| is_idr(ty(n))))
    };
    if sps && pps {
        headers.clear();
        for n in nals.iter().filter(|n| param(ty(n))) {
            annex_b::push_annex_b_nal(headers, n);
        }
    }
    if !idr {
        return (data, false);
    }
    if sps && pps || headers.is_empty() {
        return (data, true);
    }
    // the IDR without its parameter sets: the newest ones ahead of it (after an access unit
    // delimiter, which stays first)
    let mut out = Vec::with_capacity(headers.len() + data.len());
    let mut put = false;
    for n in &nals {
        if !put && ty(n) != aud {
            out.extend_from_slice(headers);
            put = true;
        }
        annex_b::push_annex_b_nal(&mut out, n);
    }
    (out, true)
}

/// The MFT's friendly name (MFT_FRIENDLY_NAME_Attribute), for the status line.
unsafe fn read_friendly_name(attributes: &IMFAttributes) -> Option<String> {
    let len = attributes.GetStringLength(&MFT_FRIENDLY_NAME_Attribute).ok()?;
    let mut buf = vec![0u16; len as usize + 1];
    let mut got = 0u32;
    attributes.GetString(&MFT_FRIENDLY_NAME_Attribute, windows::core::PWSTR(buf.as_mut_ptr()), buf.len() as u32, Some(&mut got)).ok()?;
    Some(String::from_utf16_lossy(&buf[..(got as usize).min(len as usize)]))
}

/// The output (H.264) then the input (NV12) type of an encoder MFT.
unsafe fn set_types(transform: &IMFTransform, options: &VideoStreamEncoderOptions) -> Result<(), VideoFileError> {
    let size = ((options.width as u64) << 32) | options.height as u64;
    let rate = ((options.fps as u64) << 32) | 1u64;
    let out_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(out)", e))?;
    let a = out_type.as_IMFAttributes();
    a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| hr_err("set out major type", e))?;
    a.SetGUID(&MF_MT_SUBTYPE, &subtype(options.codec)).map_err(|e| hr_err("set out subtype", e))?;
    a.SetUINT32(&MF_MT_AVG_BITRATE, options.bitrate_kbps.saturating_mul(1000)).map_err(|e| hr_err("set out bitrate", e))?;
    a.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| hr_err("set out frame size", e))?;
    a.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(|e| hr_err("set out frame rate", e))?;
    a.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| hr_err("set out interlace mode", e))?;
    // H.264 High (eAVEncH264VProfile_High = 100), H.265 Main (eAVEncH265VProfile_Main_420_8 = 1);
    // the encoder picks the level for the size and rate
    let _ = a.SetUINT32(&MF_MT_VIDEO_PROFILE, if options.codec == StreamVideoCodec::Hevc { 1 } else { 100 });
    set_bt709(a);
    transform.SetOutputType(0, &out_type, 0).map_err(|e| hr_err("IMFTransform::SetOutputType", e))?;
    let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(in)", e))?;
    let a = in_type.as_IMFAttributes();
    a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| hr_err("set in major type", e))?;
    a.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| hr_err("set in subtype", e))?;
    a.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| hr_err("set in frame size", e))?;
    a.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(|e| hr_err("set in frame rate", e))?;
    a.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| hr_err("set in interlace mode", e))?;
    set_bt709(a);
    transform.SetInputType(0, &in_type, 0).map_err(|e| hr_err("IMFTransform::SetInputType", e))
}

/// BT.709, limited range: what the GPU's conversion writes (`makepad_hal::shared_texture::to_nv12`)
/// and the VUI the viewers' decoders read.
unsafe fn set_bt709(a: &IMFAttributes) {
    let _ = a.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
    let _ = a.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
    let _ = a.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
    let _ = a.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
}

pub struct WindowsStreamEncoder {
    /// The GPU's encoder; the software MFT below only on a machine without one.
    hardware: Option<HardwareMft>,
    options: VideoStreamEncoderOptions,
    /// The software H.264 MFT: only without a hardware encoder.
    transform: Option<IMFTransform>,
    provides_samples: bool,
    output_buffer_size: u32,
    duration_100ns: i64,
    frames_since_keyframe: u32,
    force_keyframe: bool,
    pending_pts: VecDeque<i64>,
    /// Why the GPU's encoder is not used (empty when it is).
    hardware_note: String,
    /// The last surface picture's stages.
    timings: EncodeTimings,
}

impl WindowsStreamEncoder {
    fn new(options: &VideoStreamEncoderOptions) -> Result<Self, VideoFileError> {
        ensure_media_foundation()?;
        let (hardware, hardware_note) = match unsafe { HardwareMft::open(options) } {
            Ok(Some(hw)) => (Some(hw), String::new()),
            Ok(None) => (None, format!("no hardware {:?} encoder MFT takes NV12 here", options.codec)),
            Err(e) => (None, format!("the hardware encoder refused the setup: {e:?}")),
        };
        let (transform, provides_samples, output_buffer_size) = match hardware {
            Some(_) => (None, false, 0),
            // the software encoder is H.264's
            None if options.codec != StreamVideoCodec::H264 => return Err(VideoFileError::new(hardware_note)),
            None => {
                let (t, p, o) = create_and_negotiate(options)?;
                (Some(t), p, o)
            }
        };
        Ok(Self {
            hardware,
            options: *options,
            transform,
            provides_samples,
            output_buffer_size,
            duration_100ns: 10_000_000 / options.fps.max(1) as i64,
            frames_since_keyframe: 0,
            force_keyframe: true, // the first frame is always a keyframe
            pending_pts: VecDeque::new(),
            hardware_note,
            timings: EncodeTimings::default(),
        })
    }

    fn recreate(&mut self) -> Result<(), VideoFileError> {
        let (transform, provides_samples, output_buffer_size) = create_and_negotiate(&self.options)?;
        self.transform = Some(transform);
        self.provides_samples = provides_samples;
        self.output_buffer_size = output_buffer_size;
        self.frames_since_keyframe = 0;
        self.pending_pts.clear();
        Ok(())
    }

    fn push_frame_nv12(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        // COM apartment state is thread-local: `new()` initialized the MTA
        // only on the thread that constructed this encoder. Since this type
        // is `Send` (see the impl below) and callers may legitimately push
        // frames from a different thread than the one that created it (e.g.
        // `makepad-asset-ai`'s realtime session, whose worker thread is not
        // necessarily the HTTP thread), re-assert MTA membership on whatever
        // thread is calling now — idempotent and cheap (a thread already in
        // the MTA is a no-op; `MFStartup` itself only runs once, process-wide).
        ensure_media_foundation()?;
        if let Some(hw) = self.hardware.as_mut() {
            let key = std::mem::take(&mut self.force_keyframe) || (self.options.keyint > 0 && self.frames_since_keyframe >= self.options.keyint);
            self.frames_since_keyframe = if key { 1 } else { self.frames_since_keyframe + 1 };
            return unsafe {
                let texture = hw.upload(nv12)?;
                hw.encode(&texture, key, pts_100ns)
            };
        }
        let keyint = self.options.keyint.max(1);
        if self.force_keyframe || self.frames_since_keyframe >= keyint {
            self.recreate()?;
        }
        self.force_keyframe = false;
        self.frames_since_keyframe += 1;

        let sample = make_input_sample(nv12, pts_100ns, self.duration_100ns)?;
        let mut packets = Vec::new();
        let transform = self.transform.clone().ok_or_else(|| VideoFileError::new("no software encoder"))?;
        let hr = unsafe {
            transform
                .ProcessInput(0, &sample, 0)
                .err()
                .unwrap_or(HRESULT(0))
        };
        if hr.is_err() {
            // Preserve the pump's drain-before-retry policy. The retry's
            // HRESULT is reported if the transform still rejects the input.
            packets.append(&mut self.drain_available()?);
            let retry_hr = unsafe {
                transform
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
        let transform = self.transform.clone().ok_or_else(|| VideoFileError::new("no software encoder"))?;
        loop {
            let provided = if self.provides_samples {
                None
            } else {
                Some(make_output_sample(self.output_buffer_size)?)
            };
            let (hr, _status, sample) =
                unsafe { windows_mft::process_output(&transform, provided) };
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


    /// A picture straight from the HAL's shared texture `key` (`makepad_hal::shared_texture`:
    /// BGRA, its keyed mutex taken for the copy), converted to NV12 on the GPU and encoded by
    /// the hardware encoder.
    fn push_shared_texture(&mut self, key: u64, pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        ensure_media_foundation()?;
        let [w, h] = [self.options.width, self.options.height];
        let Some(hw) = self.hardware.as_mut() else {
            return Err(VideoFileError::new(format!("no hardware H.264 encoder for GPU pictures ({})", self.hardware_note)));
        };
        let t0 = std::time::Instant::now();
        let picture = makepad_hal::shared_texture::picture(key, [0, 0, w, h]).map_err(VideoFileError::new)?;
        let t1 = std::time::Instant::now();
        let nv12 = makepad_hal::shared_texture::to_nv12(&picture, w, h).map_err(VideoFileError::new)?;
        let t2 = std::time::Instant::now();
        let key_frame = std::mem::take(&mut self.force_keyframe);
        let out = unsafe { hw.encode(&nv12, key_frame, pts_100ns) };
        let us = |a: std::time::Instant, b: std::time::Instant| (b - a).as_micros().min(u32::MAX as u128) as u32;
        self.timings = EncodeTimings { copy_us: us(t0, t1), convert_us: us(t1, t2), encode_us: us(t2, std::time::Instant::now()) };
        out
    }

    /// An NV12 shared texture `key` of the encoder's size (`makepad_hal::shared_texture`): copied on
    /// the GPU into the encoder's device and encoded, no conversion, no CPU pass.
    fn push_shared_nv12(&mut self, key: u64, pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        ensure_media_foundation()?;
        let Some(hw) = self.hardware.as_mut() else {
            return Err(VideoFileError::new(format!("no hardware encoder for GPU pictures ({})", self.hardware_note)));
        };
        let t0 = std::time::Instant::now();
        let nv12 = makepad_hal::shared_texture::nv12_picture(key).map_err(VideoFileError::new)?;
        let t1 = std::time::Instant::now();
        let key_frame = std::mem::take(&mut self.force_keyframe);
        let out = unsafe { hw.encode(&nv12, key_frame, pts_100ns) };
        let us = |a: std::time::Instant, b: std::time::Instant| (b - a).as_micros().min(u32::MAX as u128) as u32;
        self.timings = EncodeTimings { copy_us: us(t0, t1), convert_us: 0, encode_us: us(t1, std::time::Instant::now()) };
        out
    }

    fn set_kbps(&mut self, kbps: u32) {
        self.options.bitrate_kbps = kbps;
        if let Some(hw) = &self.hardware {
            let _ = unsafe { windows_mft::set_codec_api_u32(&hw.transform, &CODECAPI_AVEncCommonMeanBitRate, kbps.saturating_mul(1000)) };
            let _ = unsafe { windows_mft::set_codec_api_u32(&hw.transform, &CODECAPI_AVENC_COMMON_MAX_BITRATE, kbps.saturating_mul(2000)) };
        }
    }

    fn info(&self) -> String {
        match &self.hardware {
            Some(hw) => format!("{} {}x{} {} fps {} kbit/s on {}", if hw.hevc { "H.265" } else { "H.264" }, self.options.width, self.options.height, self.options.fps, self.options.bitrate_kbps, hw.name),
            None => format!("H.264 {}x{} {} fps {} kbit/s (software encoder: {})", self.options.width, self.options.height, self.options.fps, self.options.bitrate_kbps, self.hardware_note),
        }
    }
}

impl StreamEncoderOs for WindowsStreamEncoder {
    fn caps() -> EncoderCaps {
        // two sessions encode a picture's columns at once (on two engines where the GPU has them)
        EncoderCaps { engines: 2, tiles: true, full_chroma: false, hevc: hardware_hevc() }
    }

    fn open(options: &VideoStreamEncoderOptions, _tile: StreamTile, _tuning: StreamTuning) -> Result<Self, VideoFileError> {
        Self::new(options)
    }

    fn encode_nv12(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        self.push_frame_nv12(nv12, pts_100ns)
    }

    /// A keyed-mutex surface (its keyed mutex orders the read after the frame that drew it,
    /// so `ready` is complete once submitted): a BGRA picture converted on the GPU; the column
    /// `tile` of a packed NV12 one read as the NV12 bytes it holds and encoded from those.
    fn encode_surface(&mut self, surface: &SurfaceHandle, tile: StreamTile, ready: &GpuCompletion, pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        if surface.kind != SurfaceKind::D3d11Keyed {
            return Err(VideoFileError::new(format!("the stream encoder takes keyed D3D11 surfaces, not {surface:?}")));
        }
        ready.wait(std::time::Duration::from_secs(2)).map_err(VideoFileError::new)?;
        match surface.format {
            SurfaceFormat::Bgra8 if tile.x0 == 0 && tile.y0 == 0 && tile.source_height == self.options.height => self.push_shared_texture(surface.id, pts_100ns),
            // an NV12 picture of this tile's size, rendered into its planes: encoded as it lies
            SurfaceFormat::Nv12 if tile.x0 == 0 && tile.y0 == 0 && tile.source_height == self.options.height => self.push_shared_nv12(surface.id, pts_100ns),
            SurfaceFormat::PackedNv12 if tile.y0 == 0 && tile.source_height == self.options.height => {
                let [w, h] = [self.options.width, self.options.height];
                let t0 = std::time::Instant::now();
                // texels: four bytes of a plane row each, the luma rows then the chroma rows
                let nv12 = makepad_hal::shared_texture::read(surface.id, [tile.x0 / 4, 0, w / 4, h + h / 2]).map_err(VideoFileError::new)?;
                let t1 = std::time::Instant::now();
                let out = self.push_frame_nv12(&nv12, pts_100ns);
                let us = |a: std::time::Instant, b: std::time::Instant| (b - a).as_micros().min(u32::MAX as u128) as u32;
                self.timings = EncodeTimings { copy_us: us(t0, t1), convert_us: 0, encode_us: us(t1, std::time::Instant::now()) };
                out
            }
            _ => Err(VideoFileError::new(format!("the stream encoder takes whole BGRA or NV12 surfaces or columns of packed NV12 ones, not {tile:?} of {surface:?}"))),
        }
    }

    fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    fn set_bitrate_kbps(&mut self, kbps: u32) {
        self.set_kbps(kbps);
    }

    fn timings(&self) -> EncodeTimings {
        self.timings
    }

    fn description(&self) -> String {
        self.info()
    }
}

// SAFETY: the encoder is accessed exclusively. Each call re-establishes MTA
// membership in ensure_media_foundation before using the MTA-created transform.
unsafe impl Send for WindowsStreamEncoder {}

#[cfg(test)]
mod tests {
    use super::*;

    fn au(types: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        for t in types {
            v.extend_from_slice(&[0, 0, 0, 1, 0x60 | t, 0xaa, *t]);
        }
        v
    }

    #[test]
    fn every_idr_carries_its_parameter_sets() {
        let mut headers = Vec::new();
        let (first, key) = in_band_headers(&mut headers, au(&[7, 8, 5]), false);
        assert!(key);
        assert_eq!(first, au(&[7, 8, 5]));
        let (p, key) = in_band_headers(&mut headers, au(&[1]), false);
        assert!(!key);
        assert_eq!(p, au(&[1]));
        // a forced IDR without SPS/PPS gets the stream's
        let (idr, key) = in_band_headers(&mut headers, au(&[9, 5]), false);
        assert!(key);
        assert_eq!(idr, au(&[9, 7, 8, 5]));
        assert_eq!(in_band_headers(&mut headers, au(&[5]), false).0, au(&[7, 8, 5]));
        // SPS alone (no IDR) is not a key frame
        assert!(!in_band_headers(&mut headers, au(&[7, 8, 1]), false).1);
    }
}
