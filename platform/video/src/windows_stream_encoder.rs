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
}

impl HardwareMft {
    /// The first hardware H.264 encoder that takes NV12, set up for a live view, or None
    /// (no hardware encoder, or it refused the setup: `Err` says why).
    unsafe fn open(options: &VideoStreamEncoderOptions) -> Result<Option<HardwareMft>, VideoFileError> {
        use windows::Win32::Graphics::Direct3D11::ID3D11Multithread;
        let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
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
        let activate = &list[0];
        let name = read_friendly_name(activate.as_IMFAttributes()).unwrap_or_else(|| "hardware H.264 encoder".into());
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
        let codec = |key: &windows::core::GUID, value: u32, what: &str| {
            if let Err(e) = windows_mft::set_codec_api_u32(&transform, key, value) {
                eprintln!("hardware encoder: {what} not taken ({e:?})");
            }
        };
        codec(&CODECAPI_AVLowLatencyMode, 1, "low latency mode");
        codec(&CODECAPI_AVEncCommonRateControlMode, eAVEncCommonRateControlMode_LowDelayVBR.0 as u32, "low-delay VBR");
        codec(&CODECAPI_AVEncCommonMeanBitRate, options.bitrate_kbps.saturating_mul(1000), "mean bitrate");
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
        Ok(Some(HardwareMft {
            transform,
            events,
            _manager: manager,
            device,
            need_input: 0,
            provides_samples,
            out_size: stream_info.cbSize,
            size: [options.width, options.height],
            duration_100ns: 10_000_000 / options.fps.max(1) as i64,
            name,
        }))
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
                let data = normalize_to_annex_b(&raw);
                let is_key = annex_b::split_annex_b(&data).iter().any(|nal| annex_b::nal_unit_type(nal) == annex_b::NAL_TYPE_SPS);
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
    a.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| hr_err("set out subtype", e))?;
    a.SetUINT32(&MF_MT_AVG_BITRATE, options.bitrate_kbps.saturating_mul(1000)).map_err(|e| hr_err("set out bitrate", e))?;
    a.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| hr_err("set out frame size", e))?;
    a.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(|e| hr_err("set out frame rate", e))?;
    a.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| hr_err("set out interlace mode", e))?;
    transform.SetOutputType(0, &out_type, 0).map_err(|e| hr_err("IMFTransform::SetOutputType", e))?;
    let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType(in)", e))?;
    let a = in_type.as_IMFAttributes();
    a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| hr_err("set in major type", e))?;
    a.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(|e| hr_err("set in subtype", e))?;
    a.SetUINT64(&MF_MT_FRAME_SIZE, size).map_err(|e| hr_err("set in frame size", e))?;
    a.SetUINT64(&MF_MT_FRAME_RATE, rate).map_err(|e| hr_err("set in frame rate", e))?;
    a.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(|e| hr_err("set in interlace mode", e))?;
    transform.SetInputType(0, &in_type, 0).map_err(|e| hr_err("IMFTransform::SetInputType", e))
}

pub struct WindowsStreamEncoder {
    /// The GPU's encoder; the software MFT below only on a machine without one.
    hardware: Option<HardwareMft>,
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
        let hardware = unsafe { HardwareMft::open(options) }.unwrap_or_else(|e| {
            eprintln!("hardware H.264 encoder unavailable: {e:?}; the software encoder takes over");
            None
        });
        let (transform, provides_samples, output_buffer_size) = create_and_negotiate(options)?;
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

    /// A picture straight from the HAL's shared texture `key` (`makepad_hal::shared_texture`:
    /// BGRA, its keyed mutex taken for the copy), converted to NV12 on the GPU and encoded by
    /// the hardware encoder.
    pub fn push_shared_texture(&mut self, key: u64, pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        ensure_media_foundation()?;
        let [w, h] = [self.options.width, self.options.height];
        let Some(hw) = self.hardware.as_mut() else {
            return Err(VideoFileError::new("no hardware H.264 encoder on this machine for GPU pictures"));
        };
        let picture = makepad_hal::shared_texture::picture(key, [0, 0, w, h]).map_err(VideoFileError::new)?;
        let nv12 = makepad_hal::shared_texture::to_nv12(&picture, w, h).map_err(VideoFileError::new)?;
        let key_frame = std::mem::take(&mut self.force_keyframe);
        unsafe { hw.encode(&nv12, key_frame, pts_100ns) }
    }

    pub fn set_bitrate_kbps(&mut self, kbps: u32) {
        self.options.bitrate_kbps = kbps;
        if let Some(hw) = &self.hardware {
            let _ = unsafe { windows_mft::set_codec_api_u32(&hw.transform, &CODECAPI_AVEncCommonMeanBitRate, kbps.saturating_mul(1000)) };
        }
    }

    pub fn description(&self) -> String {
        match &self.hardware {
            Some(hw) => format!("H.264 {}x{} {} fps {} kbit/s on {}", self.options.width, self.options.height, self.options.fps, self.options.bitrate_kbps, hw.name),
            None => format!("H.264 {}x{} {} fps {} kbit/s (software encoder)", self.options.width, self.options.height, self.options.fps, self.options.bitrate_kbps),
        }
    }
}

// SAFETY: the encoder is accessed exclusively. Each call re-establishes MTA
// membership in ensure_media_foundation before using the MTA-created transform.
unsafe impl Send for WindowsStreamEncoder {}
