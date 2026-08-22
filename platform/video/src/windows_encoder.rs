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
            IMFTransform, ICodecAPI, CODECAPI_AVEncMPVGOPSize,
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
        Win32::System::Variant::{VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0},
    },
    std::sync::atomic::{AtomicI32, Ordering},
    std::sync::Once,
};

const HNS_PER_SECOND: u128 = 10_000_000;

/// `MF_MT_MAX_KEYFRAME_SPACING` (missing from the pinned windows crate
/// version): max frames between key frames; 1 = all-intra.
const MF_MT_MAX_KEYFRAME_SPACING: windows::core::GUID =
    windows::core::GUID::from_u128(0xc16eb52b_73a1_476f_8d62_839d6a020652);

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
}

impl WindowsVideoFileEncoder {
    pub fn new(path: &str, options: &VideoFileEncoderOptions) -> Result<Self, VideoFileError> {
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

            // Input (raw) video type: NV12.
            let in_type = MFCreateMediaType().map_err(|e| hr_err("MFCreateMediaType", e))?;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| hr_err("set input major type", e))?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|e| hr_err("set input subtype NV12", e))?;
            in_type
                .SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    ((options.width as u64) << 32) | options.height as u64,
                )
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
                .SetUINT32(&MF_MT_DEFAULT_STRIDE, options.width)
                .map_err(|e| hr_err("set input stride", e))?;
            sink.SetInputMediaType(video_stream, &in_type, None::<&IMFAttributes>)
                .map_err(|e| hr_err("IMFSinkWriter::SetInputMediaType(video NV12)", e))?;
            if options.keyframe_only {
                // The H.264 MFT IGNORES MF_MT_MAX_KEYFRAME_SPACING on the
                // media type (measured: 48-frame GOPs shipped regardless).
                // The binding control is ICodecAPI on the sink writer's
                // encoder — reachable only after SetInputMediaType created
                // it. Service GUID GUID_NULL selects the codec api per the
                // sink-writer contract. Failing to enforce all-intra is an
                // ERROR here, not a downgrade: callers asked for it because
                // reverse playback depends on it.
                let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
                sink.GetServiceForStream(
                    video_stream,
                    &GUID::zeroed(),
                    &ICodecAPI::IID,
                    &mut raw,
                )
                .map_err(|e| hr_err("GetServiceForStream(ICodecAPI)", e))?;
                let codec = unsafe { ICodecAPI::from_raw(raw) };
                let gop = VARIANT {
                    Anonymous: VARIANT_0 {
                        Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                            // VT_UI4 (the trimmed vendored crate carries the
                            // VARENUM type but not this constant).
                            vt: VARENUM(19),
                            wReserved1: 0,
                            wReserved2: 0,
                            wReserved3: 0,
                            Anonymous: VARIANT_0_0_0 { ulVal: 1 },
                        }),
                    },
                };
                codec
                    .SetValue(&CODECAPI_AVEncMPVGOPSize, &gop)
                    .map_err(|e| hr_err("ICodecAPI::SetValue(GOP size 1)", e))?;
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
