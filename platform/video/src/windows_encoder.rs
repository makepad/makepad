//! Windows Media Foundation backend for the video FILE encoder seam.
//!
//! Sink-writer based: raw NV12 frames (+ optional 16-bit PCM audio) in,
//! finalized H.265/H.264 + AAC MP4 on disk out. Hardware transforms are
//! enabled so the OS auto-selects whatever hardware encoder MFT exists
//! (vendor-neutral: NVENC / AMD VCN / Intel QuickSync), falling back to the
//! software MFT otherwise. Which transform engaged is reported, never
//! required.

use {
    crate::{
        nv12, PcmAudioTrackOptions, VideoFileCodec, VideoFileEncoderOptions, VideoFileError,
        VideoTransformInfo,
    },
    windows::{
        core::{Interface, GUID, PCWSTR, PWSTR},
        Win32::Media::MediaFoundation::{
            eAVEncH264VProfile_Main, eAVEncH265VProfile_Main_420_8, IMFAttributes,
            IMFByteStream, IMFMediaBuffer, IMFSample, IMFSinkWriter, IMFSinkWriterEx,
            IMFTransform, CODECAPI_AVEncMPVGOPSize,
            MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateAttributes,
            MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
            MFCreateSinkWriterFromURL, MFMediaType_Audio, MFMediaType_Video, MFStartup,
            MFTranscodeContainerType_MPEG4, MFVideoFormat_H264, MFVideoFormat_HEVC,
            MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_HARDWARE_URL_Attribute, MFT_FRIENDLY_NAME_Attribute,
            MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
            MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
            MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE,
            MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
            MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_MT_VIDEO_PROFILE,
            MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING,
            MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
        },
        Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED},
        Win32::Graphics::Direct3D11::{
            ID3D11Device, ID3D11Multithread, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
            D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
            D3D11_USAGE_DEFAULT,
        },
        Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
        Win32::Media::MediaFoundation::{MFCreateDXGIDeviceManager, IMFDXGIDeviceManager},
    },
    std::sync::atomic::{AtomicI32, Ordering},
    std::sync::Once,
};

const HNS_PER_SECOND: u128 = 10_000_000;

/// `MF_MT_MAX_KEYFRAME_SPACING` (missing from the pinned windows crate
/// version): max frames between key frames; 1 = all-intra.
const MF_MT_MAX_KEYFRAME_SPACING: windows::core::GUID =
    windows::core::GUID::from_u128(0xc16eb52b_73a1_476f_8d62_839d6a020652);

/// `MF_SINK_WRITER_D3D_MANAGER` (missing from the pinned windows crate
/// version; mfreadwrite.h gives it the source reader's value).
const MF_SINK_WRITER_D3D_MANAGER: GUID = GUID::from_u128(0xec822da2_e1e9_4b29_a0d8_563c719f5269);

/// `MFVideoFormat_RGB32` (D3DFMT_X8R8G8B8, BGRA bytes, alpha ignored),
/// missing from the pinned windows crate version.
const MF_VIDEO_FORMAT_RGB32: GUID = GUID::from_u128(0x00000016_0000_0010_8000_00aa00389b71);

#[link(name = "mfplat")]
extern "system" {
    /// Wraps a DXGI surface as a media buffer; not in the pinned bindings.
    fn MFCreateDXGISurfaceBuffer(
        riid: *const GUID,
        surface: *mut std::ffi::c_void,
        subresource_index: u32,
        bottom_up_when_linear: i32,
        buffer: *mut *mut std::ffi::c_void,
    ) -> windows::core::HRESULT;
}

/// The app's device, the textures the encoder reads (see `new_d3d11`).
struct D3d11Input {
    device: ID3D11Device,
    width: u32,
    height: u32,
    _manager: IMFDXGIDeviceManager,
    bgra_scratch: Vec<u8>,
}

pub(crate) fn hr_err(context: &str, err: windows::core::Error) -> VideoFileError {
    VideoFileError::with_code(format!("{}: {}", context, err.message()), err.code().0)
}

/// Initialize COM (MTA) on this thread and Media Foundation once per process.
/// MFShutdown is intentionally never called; MF stays up for the process
/// lifetime like the other platform media users.
pub(crate) fn ensure_media_foundation() -> Result<(), VideoFileError> {
    static ONCE: Once = Once::new();
    static STARTUP_CODE: AtomicI32 = AtomicI32::new(0);
    unsafe {
        // Per-thread; RPC_E_CHANGED_MODE (already STA) is fine — MF only
        // requires COM to be initialized, the sink writer manages its own
        // worker threads.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    ONCE.call_once(|| {
        let result = unsafe { MFStartup(MF_VERSION, 0) };
        if let Err(err) = result {
            STARTUP_CODE.store(err.code().0, Ordering::SeqCst);
        }
    });
    let code = STARTUP_CODE.load(Ordering::SeqCst);
    if code != 0 {
        return Err(VideoFileError::with_code("MFStartup failed", code));
    }
    Ok(())
}

pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Copy `bytes` into a new locked media buffer wrapped in a sample with the
/// given timestamp and duration.
fn make_sample(bytes: &[u8], pts_100ns: i64, duration_100ns: i64) -> Result<IMFSample, VideoFileError> {
    unsafe {
        let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(bytes.len() as u32)
            .map_err(|e| hr_err("MFCreateMemoryBuffer", e))?;
        let mut ptr = std::ptr::null_mut();
        buffer
            .Lock(&mut ptr, None, None)
            .map_err(|e| hr_err("IMFMediaBuffer::Lock", e))?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        buffer.Unlock().map_err(|e| hr_err("IMFMediaBuffer::Unlock", e))?;
        buffer
            .SetCurrentLength(bytes.len() as u32)
            .map_err(|e| hr_err("IMFMediaBuffer::SetCurrentLength", e))?;

        let sample: IMFSample = MFCreateSample().map_err(|e| hr_err("MFCreateSample", e))?;
        sample
            .AddBuffer(&buffer)
            .map_err(|e| hr_err("IMFSample::AddBuffer", e))?;
        sample
            .SetSampleTime(pts_100ns)
            .map_err(|e| hr_err("IMFSample::SetSampleTime", e))?;
        sample
            .SetSampleDuration(duration_100ns)
            .map_err(|e| hr_err("IMFSample::SetSampleDuration", e))?;
        Ok(sample)
    }
}

/// Read an allocated-string attribute; None when absent.
unsafe fn read_string_attribute(attributes: &IMFAttributes, key: &GUID) -> Option<String> {
    let mut value = PWSTR(std::ptr::null_mut());
    let mut len = 0u32;
    if attributes.GetAllocatedString(key, &mut value, &mut len).is_ok() {
        let out = value.to_string().ok();
        CoTaskMemFree(Some(value.0 as *const _));
        out
    } else {
        None
    }
}

pub struct WindowsVideoFileEncoder {
    sink: IMFSinkWriter,
    video_stream: u32,
    audio_stream: Option<u32>,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    audio_sample_rate: u32,
    audio_channels: u16,
    frame_index: u64,
    audio_frames_pushed: u64,
    nv12_scratch: Vec<u8>,
    transform_info: Option<VideoTransformInfo>,
    finalized: bool,
    d3d11: Option<D3d11Input>,
}

impl WindowsVideoFileEncoder {
    pub fn new(path: &str, options: &VideoFileEncoderOptions) -> Result<Self, VideoFileError> {
        Self::new_with(path, options, None)
    }

    /// An encoder fed BGRA8 textures of `source_width` x `source_height` on
    /// `device` (`push_d3d11_texture`), scaled to the track's size and
    /// converted to the codec's YUV on the GPU by Media Foundation's video
    /// processor: no CPU readback or conversion. Turns on the device's
    /// multithread protection (Media Foundation uses it from its threads).
    pub fn new_d3d11(
        path: &str,
        options: &VideoFileEncoderOptions,
        device: &ID3D11Device,
        source_width: u32,
        source_height: u32,
    ) -> Result<Self, VideoFileError> {
        Self::new_with(path, options, Some((device, source_width, source_height)))
    }

    fn new_with(
        path: &str,
        options: &VideoFileEncoderOptions,
        d3d11: Option<(&ID3D11Device, u32, u32)>,
    ) -> Result<Self, VideoFileError> {
        ensure_media_foundation()?;
        unsafe {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 3).map_err(|e| hr_err("MFCreateAttributes", e))?;
            let attributes = attributes.unwrap();
            attributes
                .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
                .map_err(|e| hr_err("set MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS", e))?;
            attributes
                .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
                .map_err(|e| hr_err("set MF_SINK_WRITER_DISABLE_THROTTLING", e))?;
            attributes
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
                .map_err(|e| hr_err("set MF_TRANSCODE_CONTAINERTYPE", e))?;
            let d3d11 = match d3d11 {
                Some((device, width, height)) => {
                    if let Ok(multithread) = device.cast::<ID3D11Multithread>() {
                        let _ = multithread.SetMultithreadProtected(true);
                    }
                    let mut reset_token = 0u32;
                    let mut manager: Option<IMFDXGIDeviceManager> = None;
                    MFCreateDXGIDeviceManager(&mut reset_token, &mut manager)
                        .map_err(|e| hr_err("MFCreateDXGIDeviceManager", e))?;
                    let manager = manager
                        .ok_or_else(|| VideoFileError::new("MFCreateDXGIDeviceManager returned nothing"))?;
                    manager
                        .ResetDevice(device, reset_token)
                        .map_err(|e| hr_err("IMFDXGIDeviceManager::ResetDevice", e))?;
                    attributes
                        .SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)
                        .map_err(|e| hr_err("set MF_SINK_WRITER_D3D_MANAGER", e))?;
                    Some(D3d11Input {
                        device: device.clone(),
                        width,
                        height,
                        _manager: manager,
                        bgra_scratch: Vec::new(),
                    })
                }
                None => None,
            };
            // Input frames: NV12 bytes, or BGRA textures at the source's size.
            let (in_subtype, in_width, in_height, in_stride) = match &d3d11 {
                Some(input) => (&MF_VIDEO_FORMAT_RGB32, input.width, input.height, input.width * 4),
                None => (&MFVideoFormat_NV12, options.width, options.height, options.width),
            };

            let wide_path = to_wide(path);
            let sink = MFCreateSinkWriterFromURL(
                PCWSTR(wide_path.as_ptr()),
                None::<&IMFByteStream>,
                &attributes,
            )
            .map_err(|e| hr_err("MFCreateSinkWriterFromURL", e))?;

            // Output (encoded) video type.
            let out_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType", e))?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| hr_err("set video major type", e))?;
            let (subtype, profile) = match options.codec {
                VideoFileCodec::H265 => (&MFVideoFormat_HEVC, eAVEncH265VProfile_Main_420_8.0),
                VideoFileCodec::H264 => (&MFVideoFormat_H264, eAVEncH264VProfile_Main.0),
            };
            out_type
                .SetGUID(&MF_MT_SUBTYPE, subtype)
                .map_err(|e| hr_err("set video subtype", e))?;
            out_type
                .SetUINT32(&MF_MT_AVG_BITRATE, options.video_bitrate_bps)
                .map_err(|e| hr_err("set video bitrate", e))?;
            out_type
                .SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    ((options.width as u64) << 32) | options.height as u64,
                )
                .map_err(|e| hr_err("set video frame size", e))?;
            out_type
                .SetUINT64(
                    &MF_MT_FRAME_RATE,
                    ((options.fps_num as u64) << 32) | options.fps_den as u64,
                )
                .map_err(|e| hr_err("set video frame rate", e))?;
            out_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| hr_err("set video interlace mode", e))?;
            out_type
                .SetUINT32(&MF_MT_VIDEO_PROFILE, profile as u32)
                .map_err(|e| hr_err("set video profile", e))?;
            if options.keyframe_only {
                // GOP size 1: every output frame is an IDR frame, so the
                // file decodes at any frame in any order (bounce loops).
                out_type
                    .SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, 1)
                    .map_err(|e| hr_err("set keyframe spacing", e))?;
            }
            let video_stream = sink
                .AddStream(&out_type)
                .map_err(|e| hr_err("IMFSinkWriter::AddStream(video)", e))?;

            // Input (raw) video type.
            let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType", e))?;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| hr_err("set input major type", e))?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, in_subtype)
                .map_err(|e| hr_err("set input subtype", e))?;
            in_type
                .SetUINT64(&MF_MT_FRAME_SIZE, ((in_width as u64) << 32) | in_height as u64)
                .map_err(|e| hr_err("set input frame size", e))?;
            in_type
                .SetUINT64(
                    &MF_MT_FRAME_RATE,
                    ((options.fps_num as u64) << 32) | options.fps_den as u64,
                )
                .map_err(|e| hr_err("set input frame rate", e))?;
            in_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| hr_err("set input interlace mode", e))?;
            in_type
                .SetUINT32(&MF_MT_DEFAULT_STRIDE, in_stride)
                .map_err(|e| hr_err("set input stride", e))?;
            if options.keyframe_only {
                // MEASURED on this MFT: every route that pokes ICodecAPI from
                // outside — GetServiceForStream before SetInputMediaType,
                // before BeginWriting, after BeginWriting, or the encoder MFT
                // straight from GetTransformForStream — returns S_OK, reads
                // the value back as 1, and still ships 48-frame GOPs. The sink
                // writer owns the encoder's configuration order, so the only
                // control that binds is the one IT applies: codec-api
                // properties handed to SetInputMediaType as
                // pEncodingParameters. Failing here is an ERROR, not a
                // downgrade — callers asked for all-intra because reverse and
                // bounce playback depend on every frame being a sync sample.
                let mut enc_params = None;
                MFCreateAttributes(&mut enc_params, 1)
                    .map_err(|e| hr_err("MFCreateAttributes(encoder params)", e))?;
                let enc_params = enc_params.unwrap();
                enc_params
                    .SetUINT32(&CODECAPI_AVEncMPVGOPSize, 1)
                    .map_err(|e| hr_err("set CODECAPI_AVEncMPVGOPSize", e))?;
                sink.SetInputMediaType(video_stream, &in_type, &enc_params)
                    .map_err(|e| {
                        hr_err("IMFSinkWriter::SetInputMediaType(video NV12, GOP 1)", e)
                    })?;
            } else {
                sink.SetInputMediaType(video_stream, &in_type, None::<&IMFAttributes>)
                    .map_err(|e| hr_err("IMFSinkWriter::SetInputMediaType(video NV12)", e))?;
            }

            // Optional audio track: PCM in, AAC out.
            let mut audio_stream = None;
            let mut audio_sample_rate = 0;
            let mut audio_channels = 0;
            if let Some(audio) = &options.audio {
                audio_stream = Some(Self::add_audio_stream(&sink, audio)?);
                audio_sample_rate = audio.sample_rate;
                audio_channels = audio.channels;
            }

            sink.BeginWriting()
                .map_err(|e| hr_err("IMFSinkWriter::BeginWriting", e))?;

            let transform_info = Self::resolve_video_transform(&sink, video_stream);
            match &transform_info {
                Some(info) => eprintln!(
                    "video file encoder: {} video transform '{}' (hardware: {})",
                    match options.codec {
                        VideoFileCodec::H265 => "H.265",
                        VideoFileCodec::H264 => "H.264",
                    },
                    info.name,
                    info.is_hardware
                ),
                None => eprintln!("video file encoder: video transform not reported by sink writer"),
            }

            Ok(Self {
                sink,
                video_stream,
                audio_stream,
                width: options.width,
                height: options.height,
                fps_num: options.fps_num,
                fps_den: options.fps_den,
                audio_sample_rate,
                audio_channels,
                frame_index: 0,
                audio_frames_pushed: 0,
                nv12_scratch: Vec::new(),
                transform_info,
                finalized: false,
                d3d11,
            })
        }
    }

    unsafe fn add_audio_stream(
        sink: &IMFSinkWriter,
        audio: &PcmAudioTrackOptions,
    ) -> Result<u32, VideoFileError> {
        // The MF AAC encoder accepts 12000/16000/20000/24000 bytes/sec
        // (96/128/160/192 kbps); snap to the nearest.
        let target_bytes = audio.aac_bitrate_bps / 8;
        let aac_bytes_per_second = *[12000u32, 16000, 20000, 24000]
            .iter()
            .min_by_key(|v| v.abs_diff(target_bytes))
            .unwrap();

        let out_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType", e))?;
        out_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| hr_err("set audio major type", e))?;
        out_type
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
            .map_err(|e| hr_err("set audio subtype AAC", e))?;
        out_type
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, audio.sample_rate)
            .map_err(|e| hr_err("set audio sample rate", e))?;
        out_type
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, audio.channels as u32)
            .map_err(|e| hr_err("set audio channels", e))?;
        out_type
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|e| hr_err("set audio bits per sample", e))?;
        out_type
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, aac_bytes_per_second)
            .map_err(|e| hr_err("set audio bitrate", e))?;
        let audio_stream = sink
            .AddStream(&out_type)
            .map_err(|e| hr_err("IMFSinkWriter::AddStream(audio)", e))?;

        let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType", e))?;
        let block_align = audio.channels as u32 * 2;
        in_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| hr_err("set audio input major type", e))?;
        in_type
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(|e| hr_err("set audio input subtype PCM", e))?;
        in_type
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, audio.sample_rate)
            .map_err(|e| hr_err("set audio input sample rate", e))?;
        in_type
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, audio.channels as u32)
            .map_err(|e| hr_err("set audio input channels", e))?;
        in_type
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|e| hr_err("set audio input bits per sample", e))?;
        in_type
            .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block_align)
            .map_err(|e| hr_err("set audio input block alignment", e))?;
        in_type
            .SetUINT32(
                &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                audio.sample_rate * block_align,
            )
            .map_err(|e| hr_err("set audio input avg bytes per second", e))?;
        in_type
            .SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
            .map_err(|e| hr_err("set audio input samples independent", e))?;
        sink.SetInputMediaType(audio_stream, &in_type, None::<&IMFAttributes>)
            .map_err(|e| hr_err("IMFSinkWriter::SetInputMediaType(audio PCM)", e))?;
        Ok(audio_stream)
    }

    /// Walk the sink writer's transform chain for the video stream and report
    /// the encoder MFT: its friendly name (when exposed) and whether it is a
    /// hardware transform (MFT_ENUM_HARDWARE_URL_Attribute present).
    fn resolve_video_transform(sink: &IMFSinkWriter, video_stream: u32) -> Option<VideoTransformInfo> {
        unsafe {
            let sink_ex: IMFSinkWriterEx = sink.cast().ok()?;
            for transform_index in 0.. {
                let mut category = GUID::zeroed();
                let mut transform: Option<IMFTransform> = None;
                if sink_ex
                    .GetTransformForStream(
                        video_stream,
                        transform_index,
                        Some(&mut category),
                        &mut transform,
                    )
                    .is_err()
                {
                    break;
                }
                let Some(transform) = transform else {
                    break;
                };
                if category != MFT_CATEGORY_VIDEO_ENCODER {
                    continue;
                }
                let Ok(attributes) = transform.GetAttributes() else {
                    return Some(VideoTransformInfo {
                        name: "unknown video encoder MFT".into(),
                        is_hardware: false,
                    });
                };
                let is_hardware =
                    read_string_attribute(&attributes, &MFT_ENUM_HARDWARE_URL_Attribute).is_some();
                let name = read_string_attribute(&attributes, &MFT_FRIENDLY_NAME_Attribute)
                    .unwrap_or_else(|| "unnamed video encoder MFT".into());
                return Some(VideoTransformInfo { name, is_hardware });
            }
            None
        }
    }

    pub fn video_transform(&self) -> Option<&VideoTransformInfo> {
        self.transform_info.as_ref()
    }

    fn frame_pts(&self, index: u64) -> i64 {
        (index as u128 * HNS_PER_SECOND * self.fps_den as u128 / self.fps_num as u128) as i64
    }

    pub fn push_frame_rgb(
        &mut self,
        rgb: &[u8],
        pixel_stride: usize,
        pts_100ns: Option<i64>,
    ) -> Result<(), VideoFileError> {
        let mut scratch = std::mem::take(&mut self.nv12_scratch);
        nv12::rgbx_to_nv12(rgb, self.width, self.height, pixel_stride, &mut scratch);
        let result = self.push_frame_nv12(&scratch, pts_100ns);
        self.nv12_scratch = scratch;
        result
    }

    pub fn push_frame_nv12(
        &mut self,
        nv12_bytes: &[u8],
        pts_100ns: Option<i64>,
    ) -> Result<(), VideoFileError> {
        let auto_pts = self.frame_pts(self.frame_index);
        let duration = self.frame_pts(self.frame_index + 1) - auto_pts;
        let pts = pts_100ns.unwrap_or(auto_pts);
        let sample = make_sample(nv12_bytes, pts, duration)?;
        unsafe {
            self.sink
                .WriteSample(self.video_stream, &sample)
                .map_err(|e| hr_err("IMFSinkWriter::WriteSample(video)", e))?;
        }
        self.frame_index += 1;
        Ok(())
    }

    /// Encode `texture` (BGRA8, the size `new_d3d11` was given, on its
    /// device) as the next frame. The sample keeps a reference to the texture
    /// until Media Foundation is done with it; the caller sees that as the
    /// texture's reference count and must not write it before then.
    pub fn push_d3d11_texture(
        &mut self,
        texture: &ID3D11Texture2D,
        pts_100ns: Option<i64>,
    ) -> Result<(), VideoFileError> {
        let Some(input) = &self.d3d11 else {
            return Err(VideoFileError::new("encoder was not opened for D3D11 textures"));
        };
        let length = input.width * input.height * 4;
        let auto_pts = self.frame_pts(self.frame_index);
        let duration = self.frame_pts(self.frame_index + 1) - auto_pts;
        let pts = pts_100ns.unwrap_or(auto_pts);
        unsafe {
            let mut raw = std::ptr::null_mut();
            MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture.as_raw(), 0, 0, &mut raw)
                .ok()
                .map_err(|e| hr_err("MFCreateDXGISurfaceBuffer", e))?;
            let buffer = IMFMediaBuffer::from_raw(raw);
            buffer
                .SetCurrentLength(length)
                .map_err(|e| hr_err("IMFMediaBuffer::SetCurrentLength", e))?;
            let sample: IMFSample = MFCreateSample().map_err(|e| hr_err("MFCreateSample", e))?;
            sample.AddBuffer(&buffer).map_err(|e| hr_err("IMFSample::AddBuffer", e))?;
            sample.SetSampleTime(pts).map_err(|e| hr_err("IMFSample::SetSampleTime", e))?;
            sample
                .SetSampleDuration(duration)
                .map_err(|e| hr_err("IMFSample::SetSampleDuration", e))?;
            self.sink
                .WriteSample(self.video_stream, &sample)
                .map_err(|e| hr_err("IMFSinkWriter::WriteSample(video texture)", e))?;
        }
        self.frame_index += 1;
        Ok(())
    }

    /// A frame of RGBA bytes for an encoder opened by `new_d3d11` (the
    /// source's size): uploaded to a texture and encoded like the others.
    pub fn push_d3d11_rgba8(&mut self, rgba: &[u8], pts_100ns: Option<i64>) -> Result<(), VideoFileError> {
        let Some(input) = &mut self.d3d11 else {
            return Err(VideoFileError::new("encoder was not opened for D3D11 textures"));
        };
        let (width, height) = (input.width, input.height);
        if rgba.len() != width as usize * height as usize * 4 {
            return Err(VideoFileError::new(format!(
                "rgba frame size {} != expected {}x{}x4",
                rgba.len(),
                width,
                height
            )));
        }
        input.bgra_scratch.clear();
        input
            .bgra_scratch
            .extend(rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]));
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: input.bgra_scratch.as_ptr() as *const _,
            SysMemPitch: width * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            input
                .device
                .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
                .map_err(|e| hr_err("CreateTexture2D(encoder frame)", e))?;
        }
        let texture = texture.ok_or_else(|| VideoFileError::new("CreateTexture2D returned nothing"))?;
        self.push_d3d11_texture(&texture, pts_100ns)
    }

    pub fn push_audio_i16(&mut self, samples: &[i16]) -> Result<(), VideoFileError> {
        let Some(audio_stream) = self.audio_stream else {
            return Err(VideoFileError::new("encoder has no audio stream"));
        };
        if samples.is_empty() {
            return Ok(());
        }
        let frames = (samples.len() / self.audio_channels as usize) as u64;
        let pts = (self.audio_frames_pushed as u128 * HNS_PER_SECOND
            / self.audio_sample_rate as u128) as i64;
        let duration = ((self.audio_frames_pushed + frames) as u128 * HNS_PER_SECOND
            / self.audio_sample_rate as u128) as i64
            - pts;
        let bytes = unsafe {
            std::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 2)
        };
        let sample = make_sample(bytes, pts, duration)?;
        unsafe {
            self.sink
                .WriteSample(audio_stream, &sample)
                .map_err(|e| hr_err("IMFSinkWriter::WriteSample(audio)", e))?;
        }
        self.audio_frames_pushed += frames;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), VideoFileError> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;
        unsafe {
            self.sink
                .Finalize()
                .map_err(|e| hr_err("IMFSinkWriter::Finalize", e))
        }
    }
}

impl Drop for WindowsVideoFileEncoder {
    fn drop(&mut self) {
        if !self.finalized {
            self.finalized = true;
            unsafe {
                let _ = self.sink.Finalize();
            }
        }
    }
}
