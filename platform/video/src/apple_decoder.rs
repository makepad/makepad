//! macOS AVFoundation backend for the video FILE decoder seam.
//!
//! AVAssetReader based: an mp4 path in, decoded NV12 frames + 16-bit PCM
//! audio out. AVAssetReaderTrackOutput decodes through VideoToolbox, which
//! uses the hardware decode engine on Apple Silicon for both H.264 and
//! H.265. Output is requested as kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
//! (NV12), matching the facade's frame type; CoreVideo hands back buffers at
//! display size (codec padding lives past the row stride), so the repack to a
//! tight buffer doubles as the aperture crop.

use {
    crate::{
        DecodedAudioChunk, DecodedVideoFrame, VideoFileCodec, VideoFileError, VideoFileInfo,
    },
    makepad_apple_sys::*,
    std::ffi::c_void,
    std::ptr::NonNull,
};

const HNS_PER_SECOND: i128 = 10_000_000;

/// RAII NSAutoreleasePool: decode pulls run on plain Rust threads which have
/// no ambient pool, and AVFoundation autoreleases freely internally.
pub(crate) struct AutoreleasePool(ObjcId);

impl AutoreleasePool {
    pub(crate) fn new() -> Self {
        unsafe { Self(msg_send![class!(NSAutoreleasePool), new]) }
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.0, release];
        }
    }
}

pub(crate) fn cmtime_to_100ns(time: CMTime) -> i64 {
    if time.flags & kCMTimeFlags_Valid == 0 || time.timescale <= 0 {
        return 0;
    }
    (time.value as i128 * HNS_PER_SECOND / time.timescale as i128) as i64
}

/// Extract "<localizedDescription> (os error <code>)" from an NSError.
pub(crate) fn nserror_to_video_error(context: &str, error: ObjcId) -> VideoFileError {
    if error == nil {
        return VideoFileError::new(context.to_string());
    }
    unsafe {
        let desc: ObjcId = msg_send![error, localizedDescription];
        let code: isize = msg_send![error, code];
        let msg = if desc != nil {
            format!("{}: {}", context, nsstring_to_string(desc))
        } else {
            context.to_string()
        };
        VideoFileError::with_code(msg, code as i32)
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.max(1)
}

/// AVAssetReaderStatus values.
const READER_STATUS_FAILED: i64 = 3;

pub struct MacosVideoFileDecoder {
    // Held to keep the object graph alive; the reader/outputs do the work.
    _asset: RcObjcId,
    _reader: RcObjcId,
    reader_id: ObjcId,
    video_output: RcObjcId,
    audio_output: Option<RcObjcId>,
    info: VideoFileInfo,
    video_eos: bool,
    audio_eos: bool,
}

// The decoder is pulled from one thread at a time; the ObjC objects it wraps
// are not thread-affine (AVAssetReader is documented safe for serial use off
// the main thread).
unsafe impl Send for MacosVideoFileDecoder {}

impl MacosVideoFileDecoder {
    pub fn open(path: &str) -> Result<Self, VideoFileError> {
        if !std::path::Path::new(path).is_file() {
            return Err(VideoFileError::new(format!("file not found: {}", path)));
        }
        let _pool = AutoreleasePool::new();
        unsafe {
            let ns_path = str_to_nsstring(path);
            let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: ns_path];
            let asset: ObjcId = msg_send![class!(AVURLAsset), URLAssetWithURL: url options: nil];
            if asset == nil {
                return Err(VideoFileError::new(format!(
                    "AVURLAsset failed to open {}",
                    path
                )));
            }

            // --- Video track (required, mirrors "no video stream in file"). ---
            let video_tracks: ObjcId = msg_send![asset, tracksWithMediaType: AVMediaTypeVideo];
            let video_track_count: usize = msg_send![video_tracks, count];
            if video_track_count == 0 {
                return Err(VideoFileError::new("no video stream in file"));
            }
            let video_track: ObjcId = msg_send![video_tracks, objectAtIndex: 0usize];

            // Compressed codec + coded dimensions from the format description.
            let mut video_codec = None;
            let mut video_codec_fourcc = 0u32;
            let mut coded_width = 0u32;
            let mut coded_height = 0u32;
            let format_descs: ObjcId = msg_send![video_track, formatDescriptions];
            let format_desc_count: usize = msg_send![format_descs, count];
            if format_desc_count > 0 {
                let desc: CMFormatDescriptionRef =
                    msg_send![format_descs, objectAtIndex: 0usize];
                video_codec_fourcc = CMFormatDescriptionGetMediaSubType(desc);
                video_codec = match video_codec_fourcc {
                    f if f == four_char_as_u32("hvc1") || f == four_char_as_u32("hev1") => {
                        Some(VideoFileCodec::H265)
                    }
                    f if f == four_char_as_u32("avc1") || f == four_char_as_u32("avc3") => {
                        Some(VideoFileCodec::H264)
                    }
                    _ => None,
                };
                let dims = CMVideoFormatDescriptionGetDimensions(desc);
                coded_width = dims.width.max(0) as u32;
                coded_height = dims.height.max(0) as u32;
            }

            // Display picture size: the container's natural size is the
            // display aperture (the coded surface may be block/CTU padded).
            let natural: NSSize = msg_send![video_track, naturalSize];
            let mut width = natural.width.round().max(0.0) as u32;
            let mut height = natural.height.round().max(0.0) as u32;
            if width == 0 || height == 0 {
                width = coded_width;
                height = coded_height;
            } else if coded_width != 0 && coded_height != 0 {
                width = width.min(coded_width.max(width));
                height = height.min(coded_height.max(height));
            }
            if width == 0 || height == 0 {
                return Err(VideoFileError::new("video track reports zero size"));
            }

            // Frame rate: minFrameDuration is an exact rational for CFR
            // content; fall back to nominalFrameRate, then 30/1.
            let min_dur: CMTime = msg_send![video_track, minFrameDuration];
            let (mut fps_num, mut fps_den) =
                if min_dur.flags & kCMTimeFlags_Valid != 0 && min_dur.value > 0 && min_dur.timescale > 0 {
                    (min_dur.timescale as u32, min_dur.value as u32)
                } else {
                    let nominal: f32 = msg_send![video_track, nominalFrameRate];
                    if nominal > 0.0 {
                        ((nominal * 1000.0).round() as u32, 1000)
                    } else {
                        (30, 1)
                    }
                };
            let g = gcd(fps_num, fps_den);
            fps_num /= g;
            fps_den /= g;

            let duration: CMTime = msg_send![asset, duration];
            let duration_100ns = cmtime_to_100ns(duration);

            // --- Reader + NV12 video output. ---
            let mut error: ObjcId = nil;
            let reader: ObjcId =
                msg_send![class!(AVAssetReader), assetReaderWithAsset: asset error: &mut error];
            if reader == nil {
                return Err(nserror_to_video_error("AVAssetReader init", error));
            }

            let nv12_number: ObjcId = msg_send![
                class!(NSNumber),
                numberWithUnsignedInt: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            ];
            let keys: &[ObjcId] = &[kCVPixelBufferPixelFormatTypeKey as ObjcId];
            let values: &[ObjcId] = &[nv12_number];
            let video_settings: ObjcId = msg_send![
                class!(NSDictionary),
                dictionaryWithObjects: values.as_ptr()
                forKeys: keys.as_ptr()
                count: 1usize
            ];
            let video_output: ObjcId = msg_send![class!(AVAssetReaderTrackOutput), alloc];
            let video_output: ObjcId = msg_send![
                video_output,
                initWithTrack: video_track
                outputSettings: video_settings
            ];
            if video_output == nil {
                return Err(VideoFileError::new(
                    "AVAssetReaderTrackOutput(video NV12) init failed",
                ));
            }
            // We repack into our own buffer before releasing the sample, so
            // the vended buffer may be recycled.
            let _: () = msg_send![video_output, setAlwaysCopiesSampleData: NO];
            let can_add: BOOL = msg_send![reader, canAddOutput: video_output];
            if can_add == NO {
                let _: () = msg_send![video_output, release];
                return Err(VideoFileError::new("AVAssetReader rejected video output"));
            }
            let _: () = msg_send![reader, addOutput: video_output];

            // --- Optional audio track -> 16-bit interleaved PCM at native
            // rate/layout. ---
            let mut has_audio = false;
            let mut audio_sample_rate = 0u32;
            let mut audio_channels = 0u16;
            let mut audio_output_obj: ObjcId = nil;
            let audio_tracks: ObjcId = msg_send![asset, tracksWithMediaType: AVMediaTypeAudio];
            let audio_track_count: usize = msg_send![audio_tracks, count];
            if audio_track_count > 0 {
                let audio_track: ObjcId = msg_send![audio_tracks, objectAtIndex: 0usize];
                // Native rate/channel count from the track's stream description.
                let mut rate = 48000u32;
                let mut channels = 2u16;
                let audio_descs: ObjcId = msg_send![audio_track, formatDescriptions];
                let audio_desc_count: usize = msg_send![audio_descs, count];
                if audio_desc_count > 0 {
                    let desc: CMFormatDescriptionRef =
                        msg_send![audio_descs, objectAtIndex: 0usize];
                    let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(desc);
                    if !asbd.is_null() {
                        let asbd = &*asbd;
                        if asbd.mSampleRate > 0.0 {
                            rate = asbd.mSampleRate as u32;
                        }
                        if asbd.mChannelsPerFrame > 0 {
                            channels = asbd.mChannelsPerFrame.min(u16::MAX as u32) as u16;
                        }
                    }
                }

                let format_number: ObjcId = msg_send![
                    class!(NSNumber),
                    numberWithUnsignedInt: AudioFormatId::LinearPCM as u32
                ];
                let bits_number: ObjcId = msg_send![class!(NSNumber), numberWithInt: 16i32];
                let no_number: ObjcId = msg_send![class!(NSNumber), numberWithBool: NO];
                let keys: &[ObjcId] = &[
                    AVFormatIDKey,
                    AVLinearPCMBitDepthKey,
                    AVLinearPCMIsFloatKey,
                    AVLinearPCMIsBigEndianKey,
                    AVLinearPCMIsNonInterleaved,
                ];
                let values: &[ObjcId] =
                    &[format_number, bits_number, no_number, no_number, no_number];
                let audio_settings: ObjcId = msg_send![
                    class!(NSDictionary),
                    dictionaryWithObjects: values.as_ptr()
                    forKeys: keys.as_ptr()
                    count: keys.len()
                ];
                let audio_output: ObjcId = msg_send![class!(AVAssetReaderTrackOutput), alloc];
                let audio_output: ObjcId = msg_send![
                    audio_output,
                    initWithTrack: audio_track
                    outputSettings: audio_settings
                ];
                if audio_output != nil {
                    let _: () = msg_send![audio_output, setAlwaysCopiesSampleData: NO];
                    let can_add: BOOL = msg_send![reader, canAddOutput: audio_output];
                    if can_add == YES {
                        let _: () = msg_send![reader, addOutput: audio_output];
                        audio_output_obj = audio_output;
                        audio_sample_rate = rate;
                        audio_channels = channels;
                        has_audio = true;
                    } else {
                        let _: () = msg_send![audio_output, release];
                    }
                }
            }

            let started: BOOL = msg_send![reader, startReading];
            if started == NO {
                let error: ObjcId = msg_send![reader, error];
                if audio_output_obj != nil {
                    let _: () = msg_send![audio_output_obj, release];
                }
                let _: () = msg_send![video_output, release];
                return Err(nserror_to_video_error("AVAssetReader startReading", error));
            }

            Ok(Self {
                _asset: RcObjcId::from_unowned(NonNull::new(asset).unwrap()),
                _reader: RcObjcId::from_unowned(NonNull::new(reader).unwrap()),
                reader_id: reader,
                // initWithTrack: returned +1; from_owned takes that reference.
                video_output: RcObjcId::from_owned(NonNull::new(video_output).unwrap()),
                audio_output: NonNull::new(audio_output_obj).map(RcObjcId::from_owned),
                info: VideoFileInfo {
                    width,
                    height,
                    fps_num,
                    fps_den,
                    duration_100ns,
                    video_codec,
                    video_codec_fourcc,
                    has_audio,
                    audio_sample_rate,
                    audio_channels,
                },
                video_eos: false,
                audio_eos: false,
            })
        }
    }

    pub fn info(&self) -> &VideoFileInfo {
        &self.info
    }

    /// Pull one sample buffer from an output; maps the null return to either
    /// EOS (`Ok(None)`) or a reader failure.
    unsafe fn copy_next_sample(
        &self,
        output: ObjcId,
        context: &str,
    ) -> Result<Option<CMSampleBufferRef>, VideoFileError> {
        let sample: CMSampleBufferRef = msg_send![output, copyNextSampleBuffer];
        if !sample.is_null() {
            return Ok(Some(sample));
        }
        let status: i64 = msg_send![self.reader_id, status];
        if status == READER_STATUS_FAILED {
            let error: ObjcId = msg_send![self.reader_id, error];
            return Err(nserror_to_video_error(context, error));
        }
        Ok(None)
    }

    pub fn next_frame(&mut self) -> Result<Option<DecodedVideoFrame>, VideoFileError> {
        if self.video_eos {
            return Ok(None);
        }
        let _pool = AutoreleasePool::new();
        unsafe {
            let Some(sample) =
                self.copy_next_sample(self.video_output.as_id(), "copyNextSampleBuffer(video)")?
            else {
                self.video_eos = true;
                return Ok(None);
            };
            let pts_100ns = cmtime_to_100ns(CMSampleBufferGetPresentationTimeStamp(sample));
            let image = CMSampleBufferGetImageBuffer(sample);
            if image.is_null() {
                CFRelease(sample as *const c_void);
                return Err(VideoFileError::new("video sample has no image buffer"));
            }

            // Repack the (possibly row-padded) biplanar surface into a tight
            // display-sized NV12 buffer. CoreVideo reports display dimensions
            // on the buffer; codec padding sits beyond bytes-per-row.
            const K_CV_PIXEL_BUFFER_LOCK_READ_ONLY: CVPixelBufferLockFlags = 1;
            CVPixelBufferLockBaseAddress(image, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);

            let buf_width = CVPixelBufferGetWidth(image);
            let buf_height = CVPixelBufferGetHeight(image);
            // Track the decoded surface if it disagrees with the container
            // (mirrors the Windows media-type-changed refresh: never report
            // more pixels than the decoder produced).
            self.info.width = self.info.width.min(buf_width.max(1) as u32).max(1);
            self.info.height = self.info.height.min(buf_height.max(1) as u32).max(1);

            let width = self.info.width as usize;
            let height = self.info.height as usize;
            let uv_height = height.div_ceil(2);
            let mut nv12 = vec![0u8; width * (height + uv_height)];

            if CVPixelBufferIsPlanar(image) && CVPixelBufferGetPlaneCount(image) >= 2 {
                let y_ptr = CVPixelBufferGetBaseAddressOfPlane(image, 0) as *const u8;
                let uv_ptr = CVPixelBufferGetBaseAddressOfPlane(image, 1) as *const u8;
                let y_stride = CVPixelBufferGetBytesPerRowOfPlane(image, 0);
                let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(image, 1);
                let src_uv_height = CVPixelBufferGetHeightOfPlane(image, 1).min(uv_height);
                if !y_ptr.is_null() && !uv_ptr.is_null() {
                    for row in 0..height {
                        let src = std::slice::from_raw_parts(y_ptr.add(row * y_stride), width);
                        nv12[row * width..row * width + width].copy_from_slice(src);
                    }
                    for row in 0..src_uv_height {
                        let src = std::slice::from_raw_parts(uv_ptr.add(row * uv_stride), width);
                        let dst = (height + row) * width;
                        nv12[dst..dst + width].copy_from_slice(src);
                    }
                }
            }

            CVPixelBufferUnlockBaseAddress(image, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
            CFRelease(sample as *const c_void);

            Ok(Some(DecodedVideoFrame {
                pts_100ns,
                width: self.info.width,
                height: self.info.height,
                nv12,
            }))
        }
    }

    pub fn next_audio(&mut self) -> Result<Option<DecodedAudioChunk>, VideoFileError> {
        if self.audio_eos || !self.info.has_audio {
            return Ok(None);
        }
        let Some(audio_output) = &self.audio_output else {
            return Ok(None);
        };
        let audio_output = audio_output.as_id();
        let _pool = AutoreleasePool::new();
        unsafe {
            let Some(sample) =
                self.copy_next_sample(audio_output, "copyNextSampleBuffer(audio)")?
            else {
                self.audio_eos = true;
                return Ok(None);
            };
            let pts_100ns = cmtime_to_100ns(CMSampleBufferGetPresentationTimeStamp(sample));
            let block = CMSampleBufferGetDataBuffer(sample);
            if block.is_null() {
                CFRelease(sample as *const c_void);
                return Err(VideoFileError::new("audio sample has no data buffer"));
            }
            let len = CMBlockBufferGetDataLength(block).max(0) as usize;
            let sample_count = len / 2;
            let mut samples = vec![0i16; sample_count];
            let status = CMBlockBufferCopyDataBytes(
                block,
                0,
                (sample_count * 2) as i32,
                samples.as_mut_ptr() as *mut c_void,
            );
            CFRelease(sample as *const c_void);
            if status != 0 {
                return Err(VideoFileError::with_code(
                    "CMBlockBufferCopyDataBytes(audio)",
                    status,
                ));
            }

            Ok(Some(DecodedAudioChunk {
                pts_100ns,
                sample_rate: self.info.audio_sample_rate,
                channels: self.info.audio_channels,
                samples,
            }))
        }
    }
}

impl Drop for MacosVideoFileDecoder {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.reader_id, cancelReading];
        }
    }
}
