//! macOS AVFoundation backend for the video FILE encoder seam.
//!
//! AVAssetWriter based: NV12 (or RGB via the facade converters) frames +
//! optional 16-bit PCM audio in, finalized mp4 on disk out. Video encodes
//! through VideoToolbox (hardware engine on Apple Silicon for both H.264 and
//! H.265); audio goes through the system AAC encoder.
//!
//! Inputs run with `expectsMediaDataInRealTime = YES`: that is the mode whose
//! documented contract lets `isReadyForMoreMediaData` be polled, which is
//! what this facade's synchronous push API needs. (With NO, readiness is only
//! ever granted through `requestMediaDataWhenReadyOnQueue:`'s pull callback —
//! polling stays NO forever.) The writer still runs faster than realtime and
//! honors the average-bitrate target; if offline rate-control quality ever
//! matters here, the fix is the queue-driven pump, not a poll.

use {
    crate::apple_decoder::{nserror_to_video_error, AutoreleasePool},
    crate::{
        nv12, VideoFileCodec, VideoFileEncoderOptions, VideoFileError, VideoTransformInfo,
    },
    makepad_apple_sys::*,
    std::ffi::c_void,
    std::ptr::NonNull,
    std::sync::atomic::{AtomicBool, Ordering},
};

/// The transform report is a per-process fact; a tape writer opens one encoder
/// per shard, so print it once.
static TRANSFORM_REPORTED: AtomicBool = AtomicBool::new(false);

const HNS_PER_SECOND: u128 = 10_000_000;
const HNS_TIMESCALE: CMTimeScale = 10_000_000;

/// AVAssetWriterStatus values.
const WRITER_STATUS_COMPLETED: i64 = 2;
const WRITER_STATUS_FAILED: i64 = 3;

fn hns_time(value_100ns: i64) -> CMTime {
    CMTime {
        value: value_100ns,
        timescale: HNS_TIMESCALE,
        flags: kCMTimeFlags_Valid,
        epoch: 0,
    }
}

unsafe fn ns_dictionary(keys: &[ObjcId], values: &[ObjcId]) -> ObjcId {
    debug_assert_eq!(keys.len(), values.len());
    msg_send![
        class!(NSDictionary),
        dictionaryWithObjects: values.as_ptr()
        forKeys: keys.as_ptr()
        count: keys.len()
    ]
}

/// Ask VideoToolbox whether a hardware-accelerated encoder exists for this
/// codec/size, and which one. `VTIsHardwareEncodeSupported` is not an
/// exported symbol on macOS; this is the public probe.
unsafe fn probe_hardware_encoder(
    width: u32,
    height: u32,
    codec_fourcc: u32,
) -> (bool, Option<String>) {
    let yes_number: ObjcId = msg_send![class!(NSNumber), numberWithBool: YES];
    let spec = ns_dictionary(
        &[kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder as ObjcId],
        &[yes_number],
    );
    let mut encoder_id: CFStringRef = std::ptr::null();
    let mut props: CFDictionaryRef = std::ptr::null();
    let status = VTCopySupportedPropertyDictionaryForEncoder(
        width as i32,
        height as i32,
        codec_fourcc,
        spec as CFDictionaryRef,
        &mut encoder_id,
        &mut props,
    );
    if !props.is_null() {
        CFRelease(props);
    }
    if status != 0 {
        return (false, None);
    }
    let name = if !encoder_id.is_null() {
        // CFString is toll-free bridged to NSString.
        let name = nsstring_to_string(encoder_id as ObjcId);
        CFRelease(encoder_id as *const c_void);
        Some(name)
    } else {
        None
    };
    (true, name)
}

/// Poll an AVAssetWriterInput until it accepts more media data. Offline
/// writes drain quickly; the timeout only guards against a wedged writer.
unsafe fn wait_for_input_ready(input: ObjcId, context: &str) -> Result<(), VideoFileError> {
    for _ in 0..20_000u32 {
        let ready: BOOL = msg_send![input, isReadyForMoreMediaData];
        if ready == YES {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    Err(VideoFileError::new(format!(
        "{}: AVAssetWriterInput never became ready",
        context
    )))
}

pub struct MacosVideoFileEncoder {
    writer: RcObjcId,
    video_input: RcObjcId,
    pixel_adaptor: RcObjcId,
    audio_input: Option<RcObjcId>,
    audio_format_desc: CMFormatDescriptionRef,
    transform_info: Option<VideoTransformInfo>,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    audio_sample_rate: u32,
    audio_channels: u16,
    frame_index: u64,
    audio_frames_pushed: u64,
    finalized: bool,
    nv12_scratch: Vec<u8>,
}

// Driven from one thread at a time; the wrapped AVFoundation objects are not
// thread-affine.
unsafe impl Send for MacosVideoFileEncoder {}

impl MacosVideoFileEncoder {
    pub fn new(path: &str, options: &VideoFileEncoderOptions) -> Result<Self, VideoFileError> {
        // AVAssetWriter refuses to write over an existing file.
        let _ = std::fs::remove_file(path);
        let _pool = AutoreleasePool::new();
        unsafe {
            let ns_path = str_to_nsstring(path);
            let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: ns_path];
            let mut error: ObjcId = nil;
            let writer: ObjcId = msg_send![
                class!(AVAssetWriter),
                assetWriterWithURL: url
                fileType: AVFileTypeMPEG4
                error: &mut error
            ];
            if writer == nil {
                return Err(nserror_to_video_error("AVAssetWriter init", error));
            }

            let (codec_type, codec_fourcc, codec_name) = match options.codec {
                VideoFileCodec::H265 => (AVVideoCodecTypeHEVC, kCMVideoCodecType_HEVC, "HEVC"),
                VideoFileCodec::H264 => (AVVideoCodecTypeH264, kCMVideoCodecType_H264, "H.264"),
            };

            // --- Video input: codec + size + bitrate. ---
            let width_number: ObjcId =
                msg_send![class!(NSNumber), numberWithUnsignedInt: options.width];
            let height_number: ObjcId =
                msg_send![class!(NSNumber), numberWithUnsignedInt: options.height];
            let bitrate_number: ObjcId = msg_send![
                class!(NSNumber),
                numberWithUnsignedInt: options.video_bitrate_bps
            ];
            let fps = (options.fps_num as f64 / options.fps_den as f64).round().max(1.0);
            let fps_number: ObjcId = msg_send![class!(NSNumber), numberWithDouble: fps];
            let compression = if options.keyframe_only {
                // Max key-frame interval 1: all-intra, so the file decodes
                // at any frame in any order (bounce loops play backwards
                // without forward-decoding a GOP).
                let one: ObjcId = msg_send![class!(NSNumber), numberWithInt: 1];
                ns_dictionary(
                    &[
                        AVVideoAverageBitRateKey,
                        AVVideoExpectedSourceFrameRateKey,
                        AVVideoMaxKeyFrameIntervalKey,
                    ],
                    &[bitrate_number, fps_number, one],
                )
            } else {
                ns_dictionary(
                    &[AVVideoAverageBitRateKey, AVVideoExpectedSourceFrameRateKey],
                    &[bitrate_number, fps_number],
                )
            };
            let video_settings = ns_dictionary(
                &[
                    AVVideoCodecKey,
                    AVVideoWidthKey,
                    AVVideoHeightKey,
                    AVVideoCompressionPropertiesKey,
                ],
                &[codec_type, width_number, height_number, compression],
            );
            let video_input: ObjcId = msg_send![
                class!(AVAssetWriterInput),
                assetWriterInputWithMediaType: AVMediaTypeVideo
                outputSettings: video_settings
            ];
            if video_input == nil {
                return Err(VideoFileError::new("AVAssetWriterInput(video) init failed"));
            }
            let _: () = msg_send![video_input, setExpectsMediaDataInRealTime: YES];
            let can_add: BOOL = msg_send![writer, canAddInput: video_input];
            if can_add == NO {
                let error: ObjcId = msg_send![writer, error];
                return Err(nserror_to_video_error(
                    "AVAssetWriter rejected video input",
                    error,
                ));
            }
            let _: () = msg_send![writer, addInput: video_input];

            // NV12 source buffers, matching the facade's frame format.
            let nv12_number: ObjcId = msg_send![
                class!(NSNumber),
                numberWithUnsignedInt: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            ];
            let source_attrs = ns_dictionary(
                &[
                    kCVPixelBufferPixelFormatTypeKey as ObjcId,
                    kCVPixelBufferWidthKey as ObjcId,
                    kCVPixelBufferHeightKey as ObjcId,
                ],
                &[nv12_number, width_number, height_number],
            );
            let pixel_adaptor: ObjcId = msg_send![
                class!(AVAssetWriterInputPixelBufferAdaptor),
                assetWriterInputPixelBufferAdaptorWithAssetWriterInput: video_input
                sourcePixelBufferAttributes: source_attrs
            ];
            if pixel_adaptor == nil {
                return Err(VideoFileError::new(
                    "AVAssetWriterInputPixelBufferAdaptor init failed",
                ));
            }

            // --- Optional audio input: PCM16 in, AAC out. ---
            let mut audio_input_obj: ObjcId = nil;
            let mut audio_format_desc: CMFormatDescriptionRef = nil;
            let mut audio_sample_rate = 0u32;
            let mut audio_channels = 0u16;
            if let Some(audio) = &options.audio {
                let format_number: ObjcId = msg_send![
                    class!(NSNumber),
                    numberWithUnsignedInt: AudioFormatId::MPEG4AAC as u32
                ];
                let rate_number: ObjcId = msg_send![
                    class!(NSNumber),
                    numberWithDouble: audio.sample_rate as f64
                ];
                let channels_number: ObjcId = msg_send![
                    class!(NSNumber),
                    numberWithUnsignedInt: audio.channels as u32
                ];
                let audio_bitrate_number: ObjcId = msg_send![
                    class!(NSNumber),
                    numberWithUnsignedInt: audio.aac_bitrate_bps
                ];
                let audio_settings = ns_dictionary(
                    &[
                        AVFormatIDKey,
                        AVSampleRateKey,
                        AVNumberOfChannelsKey,
                        AVEncoderBitRateKey,
                    ],
                    &[
                        format_number,
                        rate_number,
                        channels_number,
                        audio_bitrate_number,
                    ],
                );
                let audio_input: ObjcId = msg_send![
                    class!(AVAssetWriterInput),
                    assetWriterInputWithMediaType: AVMediaTypeAudio
                    outputSettings: audio_settings
                ];
                if audio_input == nil {
                    return Err(VideoFileError::new("AVAssetWriterInput(audio) init failed"));
                }
                let _: () = msg_send![audio_input, setExpectsMediaDataInRealTime: YES];
                let can_add: BOOL = msg_send![writer, canAddInput: audio_input];
                if can_add == NO {
                    let error: ObjcId = msg_send![writer, error];
                    return Err(nserror_to_video_error(
                        "AVAssetWriter rejected audio input",
                        error,
                    ));
                }
                let _: () = msg_send![writer, addInput: audio_input];

                // Stream description for the PCM16 sample buffers we feed in.
                let asbd = CAudioStreamBasicDescription {
                    mSampleRate: audio.sample_rate as f64,
                    mFormatID: AudioFormatId::LinearPCM,
                    mFormatFlags: LinearPcmFlags::IS_SIGNED_INTEGER as u32
                        | LinearPcmFlags::IS_PACKED as u32,
                    mBytesPerPacket: audio.channels as u32 * 2,
                    mFramesPerPacket: 1,
                    mBytesPerFrame: audio.channels as u32 * 2,
                    mChannelsPerFrame: audio.channels as u32,
                    mBitsPerChannel: 16,
                    mReserved: 0,
                };
                let status = CMAudioFormatDescriptionCreate(
                    std::ptr::null(),
                    &asbd,
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                    &mut audio_format_desc,
                );
                if status != 0 || audio_format_desc == nil {
                    return Err(VideoFileError::with_code(
                        "CMAudioFormatDescriptionCreate(pcm16)",
                        status,
                    ));
                }
                audio_input_obj = audio_input;
                audio_sample_rate = audio.sample_rate;
                audio_channels = audio.channels;
            }

            let started: BOOL = msg_send![writer, startWriting];
            if started == NO {
                let error: ObjcId = msg_send![writer, error];
                if audio_format_desc != nil {
                    CFRelease(audio_format_desc as *const c_void);
                }
                return Err(nserror_to_video_error("AVAssetWriter startWriting", error));
            }
            let _: () = msg_send![writer, startSessionAtSourceTime: hns_time(0)];

            // Which engine VideoToolbox will use, for diagnostics parity with
            // the Windows MFT report.
            let (is_hardware, encoder_id) =
                probe_hardware_encoder(options.width, options.height, codec_fourcc);
            let name = encoder_id
                .unwrap_or_else(|| format!("Apple VideoToolbox {} Encoder", codec_name));
            // One line per process, not one per encoder: a tape writer opens
            // an encoder per shard and the repeat drowned everything else out.
            if !TRANSFORM_REPORTED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "video encoder transform: '{}', hardware: {}",
                    name,
                    is_hardware
                );
            }
            let transform_info = Some(VideoTransformInfo { name, is_hardware });

            Ok(Self {
                writer: RcObjcId::from_unowned(NonNull::new(writer).unwrap()),
                video_input: RcObjcId::from_unowned(NonNull::new(video_input).unwrap()),
                pixel_adaptor: RcObjcId::from_unowned(NonNull::new(pixel_adaptor).unwrap()),
                audio_input: NonNull::new(audio_input_obj).map(RcObjcId::from_unowned),
                audio_format_desc,
                transform_info,
                width: options.width,
                height: options.height,
                fps_num: options.fps_num,
                fps_den: options.fps_den,
                audio_sample_rate,
                audio_channels,
                frame_index: 0,
                audio_frames_pushed: 0,
                finalized: false,
                nv12_scratch: Vec::new(),
            })
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
        if self.finalized {
            return Err(VideoFileError::new("encoder already finalized"));
        }
        let pts = pts_100ns.unwrap_or_else(|| self.frame_pts(self.frame_index));
        let _pool = AutoreleasePool::new();
        unsafe {
            wait_for_input_ready(self.video_input.as_id(), "push_frame_nv12")?;

            let width = self.width as usize;
            let height = self.height as usize;
            let uv_height = height.div_ceil(2);
            let mut pixel_buffer: CVPixelBufferRef = std::ptr::null_mut();
            let status = CVPixelBufferCreate(
                std::ptr::null(),
                width,
                height,
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                std::ptr::null(),
                &mut pixel_buffer,
            );
            if status != 0 || pixel_buffer.is_null() {
                return Err(VideoFileError::with_code("CVPixelBufferCreate(nv12)", status));
            }
            CVPixelBufferLockBaseAddress(pixel_buffer, 0);
            let y_ptr = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0) as *mut u8;
            let uv_ptr = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 1) as *mut u8;
            let y_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0);
            let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 1);
            if y_ptr.is_null() || uv_ptr.is_null() {
                CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
                CVPixelBufferRelease(pixel_buffer);
                return Err(VideoFileError::new("CVPixelBuffer has no plane storage"));
            }
            for row in 0..height {
                std::ptr::copy_nonoverlapping(
                    nv12_bytes.as_ptr().add(row * width),
                    y_ptr.add(row * y_stride),
                    width,
                );
            }
            for row in 0..uv_height {
                std::ptr::copy_nonoverlapping(
                    nv12_bytes.as_ptr().add((height + row) * width),
                    uv_ptr.add(row * uv_stride),
                    width,
                );
            }
            CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);

            let appended: BOOL = msg_send![
                self.pixel_adaptor.as_id(),
                appendPixelBuffer: pixel_buffer
                withPresentationTime: hns_time(pts)
            ];
            CVPixelBufferRelease(pixel_buffer);
            if appended == NO {
                let error: ObjcId = msg_send![self.writer.as_id(), error];
                return Err(nserror_to_video_error("appendPixelBuffer", error));
            }
        }
        self.frame_index += 1;
        Ok(())
    }

    pub fn push_audio_i16(&mut self, samples: &[i16]) -> Result<(), VideoFileError> {
        let Some(audio_input) = &self.audio_input else {
            return Err(VideoFileError::new("encoder has no audio stream"));
        };
        if samples.is_empty() {
            return Ok(());
        }
        if self.finalized {
            return Err(VideoFileError::new("encoder already finalized"));
        }
        let audio_input = audio_input.as_id();
        let frames = (samples.len() / self.audio_channels as usize) as u64;
        let byte_len = samples.len() * 2;
        let _pool = AutoreleasePool::new();
        unsafe {
            wait_for_input_ready(audio_input, "push_audio_i16")?;

            let mut block: CMBlockBufferRef = std::ptr::null_mut();
            let status = CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null(),
                std::ptr::null_mut(),
                byte_len,
                std::ptr::null(),
                std::ptr::null(),
                0,
                byte_len,
                0,
                &mut block,
            );
            if status != 0 || block.is_null() {
                return Err(VideoFileError::with_code(
                    "CMBlockBufferCreateWithMemoryBlock(audio)",
                    status,
                ));
            }
            let status = CMBlockBufferAssureBlockMemory(block);
            if status != 0 {
                CFRelease(block as *const c_void);
                return Err(VideoFileError::with_code(
                    "CMBlockBufferAssureBlockMemory(audio)",
                    status,
                ));
            }
            let status = CMBlockBufferReplaceDataBytes(
                samples.as_ptr() as *const c_void,
                block,
                0,
                byte_len,
            );
            if status != 0 {
                CFRelease(block as *const c_void);
                return Err(VideoFileError::with_code(
                    "CMBlockBufferReplaceDataBytes(audio)",
                    status,
                ));
            }

            let pts = CMTime {
                value: self.audio_frames_pushed as i64,
                timescale: self.audio_sample_rate as CMTimeScale,
                flags: kCMTimeFlags_Valid,
                epoch: 0,
            };
            let mut sample: CMSampleBufferRef = std::ptr::null_mut();
            let status = CMAudioSampleBufferCreateReadyWithPacketDescriptions(
                std::ptr::null(),
                block,
                self.audio_format_desc,
                frames as isize,
                pts,
                std::ptr::null(),
                &mut sample,
            );
            if status != 0 || sample.is_null() {
                CFRelease(block as *const c_void);
                return Err(VideoFileError::with_code(
                    "CMAudioSampleBufferCreateReady(audio)",
                    status,
                ));
            }

            let appended: BOOL = msg_send![audio_input, appendSampleBuffer: sample];
            CFRelease(sample as *const c_void);
            CFRelease(block as *const c_void);
            if appended == NO {
                let error: ObjcId = msg_send![self.writer.as_id(), error];
                return Err(nserror_to_video_error("appendSampleBuffer(audio)", error));
            }
        }
        self.audio_frames_pushed += frames;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), VideoFileError> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;
        let _pool = AutoreleasePool::new();
        unsafe {
            let _: () = msg_send![self.video_input.as_id(), markAsFinished];
            if let Some(audio_input) = &self.audio_input {
                let _: () = msg_send![audio_input.as_id(), markAsFinished];
            }
            let block = objc_block!(move || {});
            let _: () = msg_send![
                self.writer.as_id(),
                finishWritingWithCompletionHandler: &block
            ];
            // Poll until the writer settles (offline finalize is fast; the
            // timeout guards against a wedged writer).
            for _ in 0..30_000u32 {
                let status: i64 = msg_send![self.writer.as_id(), status];
                if status == WRITER_STATUS_COMPLETED {
                    return Ok(());
                }
                if status >= WRITER_STATUS_FAILED {
                    let error: ObjcId = msg_send![self.writer.as_id(), error];
                    return Err(nserror_to_video_error("AVAssetWriter finishWriting", error));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(VideoFileError::new("AVAssetWriter finishWriting timed out"))
        }
    }
}

impl Drop for MacosVideoFileEncoder {
    fn drop(&mut self) {
        if !self.finalized {
            let _ = self.finish();
        }
        unsafe {
            if self.audio_format_desc != nil {
                CFRelease(self.audio_format_desc as *const c_void);
            }
        }
    }
}
