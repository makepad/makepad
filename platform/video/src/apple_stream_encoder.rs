//! macOS backend for the low-latency stream encoder: `VTCompressionSession`
//! driven directly (not `AVAssetWriter`, which has no raw-NAL seam).
//!
//! Driven synchronously: `push_frame_nv12` calls `VTCompressionSessionEncode
//! Frame` then `VTCompressionSessionCompleteFrames(.., kCMTimePositiveInfinity)`
//! before returning, so by the time it returns the output callback (which
//! VideoToolbox may invoke on an internal thread, synchronously or
//! asynchronously depending on its own scheduling) has already fired for
//! this frame. This trades a little pipelining for a dead-simple "push one
//! frame, get its packets back" API — the right tradeoff for an interactive
//! realtime session pushing frames one at a time.

use crate::annex_b;
use crate::stream_encoder::{EncodedPacket, StreamVideoCodec, VideoStreamEncoderOptions};
use crate::VideoFileError;
use makepad_apple_sys::*;
use std::os::raw::c_void;
use std::sync::Mutex;

const HNS_TIMESCALE: CMTimeScale = 10_000_000;

fn hns_time(value_100ns: i64) -> CMTime {
    CMTime {
        value: value_100ns,
        timescale: HNS_TIMESCALE,
        flags: kCMTimeFlags_Valid,
        epoch: 0,
    }
}

fn cmtime_to_hns(t: CMTime) -> i64 {
    if t.timescale == 0 {
        0
    } else {
        (t.value as i128 * HNS_TIMESCALE as i128 / t.timescale as i128) as i64
    }
}

struct EncoderShared {
    packets: Vec<(Vec<u8>, i64, bool)>,
}

/// True when the sample's attachments array does NOT carry
/// `kCMSampleAttachmentKey_NotSync` — the standard VideoToolbox convention
/// for "this is a sync sample" (keyframe).
unsafe fn sample_is_keyframe(sample_buffer: CMSampleBufferRef) -> bool {
    let attachments = CMSampleBufferGetSampleAttachmentsArray(sample_buffer, false);
    if attachments.is_null() || CFArrayGetCount(attachments) == 0 {
        return true;
    }
    let dict = CFArrayGetValueAtIndex(attachments, 0) as CFDictionaryRef;
    CFDictionaryContainsKey(dict, kCMSampleAttachmentKey_NotSync as *const c_void) == 0
}

unsafe extern "C" fn encode_output_callback(
    output_callback_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: OSStatus,
    _info_flags: VTEncodeInfoFlags,
    sample_buffer: CMSampleBufferRef,
) {
    if status != 0 || sample_buffer.is_null() {
        return;
    }
    let shared = &*(output_callback_ref_con as *const Mutex<EncoderShared>);
    let is_key = sample_is_keyframe(sample_buffer);
    let pts_100ns = cmtime_to_hns(CMSampleBufferGetPresentationTimeStamp(sample_buffer));

    let mut out = Vec::new();
    if is_key {
        let format_desc = CMSampleBufferGetFormatDescription(sample_buffer);
        if !format_desc.is_null() {
            for index in 0..2usize {
                let mut ptr: *const u8 = std::ptr::null();
                let mut size: usize = 0;
                let mut count: usize = 0;
                let mut nal_header_len: i32 = 0;
                let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    format_desc,
                    index,
                    &mut ptr,
                    &mut size,
                    &mut count,
                    &mut nal_header_len,
                );
                if st == 0 && !ptr.is_null() && size > 0 {
                    let nal = std::slice::from_raw_parts(ptr, size);
                    annex_b::push_annex_b_nal(&mut out, nal);
                }
            }
        }
    }
    let block = CMSampleBufferGetDataBuffer(sample_buffer);
    if !block.is_null() {
        let len = CMBlockBufferGetDataLength(block);
        if len > 0 {
            let mut avcc = vec![0u8; len as usize];
            let st = CMBlockBufferCopyDataBytes(block, 0, len, avcc.as_mut_ptr() as *mut c_void);
            if st == 0 {
                out.extend_from_slice(&annex_b::avcc_to_annex_b(&avcc));
            }
        }
    }
    if let Ok(mut guard) = shared.lock() {
        guard.packets.push((out, pts_100ns, is_key));
    }
}

unsafe fn set_bool_property(session: VTCompressionSessionRef, key: CFStringRef, value: bool) {
    let v = if value { kCFBooleanTrue } else { kCFBooleanFalse };
    VTSessionSetProperty(session as *mut c_void, key, v as *const c_void);
}

unsafe fn set_i32_property(session: VTCompressionSessionRef, key: CFStringRef, value: i32) {
    let num = CFNumberCreate(std::ptr::null(), kCFNumberSInt32Type, &value as *const i32 as *const c_void);
    if !num.is_null() {
        VTSessionSetProperty(session as *mut c_void, key, num as *const c_void);
        CFRelease(num as *const c_void);
    }
}

pub struct AppleStreamEncoder {
    session: VTCompressionSessionRef,
    shared: Box<Mutex<EncoderShared>>,
    width: u32,
    height: u32,
    frame_duration: CMTime,
    force_keyframe: bool,
}

impl AppleStreamEncoder {
    pub fn new(options: &VideoStreamEncoderOptions) -> Result<Self, VideoFileError> {
        let codec_type = match options.codec {
            StreamVideoCodec::H264 => kCMVideoCodecType_H264,
            StreamVideoCodec::Hevc => kCMVideoCodecType_HEVC,
        };
        let shared = Box::new(Mutex::new(EncoderShared { packets: Vec::new() }));
        let refcon = shared.as_ref() as *const Mutex<EncoderShared> as *mut c_void;
        let mut session: VTCompressionSessionRef = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSessionCreate(
                std::ptr::null(),
                options.width,
                options.height,
                codec_type,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                Some(encode_output_callback),
                refcon,
                &mut session,
            )
        };
        if status != 0 || session.is_null() {
            return Err(VideoFileError::with_code("VTCompressionSessionCreate", status));
        }
        unsafe {
            set_bool_property(session, kVTCompressionPropertyKey_RealTime, true);
            set_bool_property(session, kVTCompressionPropertyKey_AllowFrameReordering, false);
            set_i32_property(session, kVTCompressionPropertyKey_MaxKeyFrameInterval, options.keyint.max(1) as i32);
            set_i32_property(
                session,
                kVTCompressionPropertyKey_AverageBitRate,
                (options.bitrate_kbps as i64 * 1000).min(i32::MAX as i64) as i32,
            );
            set_i32_property(session, kVTCompressionPropertyKey_ExpectedFrameRate, options.fps as i32);
            if matches!(options.codec, StreamVideoCodec::H264) {
                VTSessionSetProperty(
                    session as *mut c_void,
                    kVTCompressionPropertyKey_ProfileLevel,
                    kVTProfileLevel_H264_Main_AutoLevel as *const c_void,
                );
            }
            let prep_status = VTCompressionSessionPrepareToEncodeFrames(session);
            if prep_status != 0 {
                VTCompressionSessionInvalidate(session);
                CFRelease(session as *const c_void);
                return Err(VideoFileError::with_code("VTCompressionSessionPrepareToEncodeFrames", prep_status));
            }
        }
        Ok(Self {
            session,
            shared,
            width: options.width,
            height: options.height,
            frame_duration: CMTime {
                value: 1,
                timescale: options.fps.max(1) as CMTimeScale,
                flags: kCMTimeFlags_Valid,
                epoch: 0,
            },
            force_keyframe: false,
        })
    }

    /// Copies `nv12` into a freshly allocated `CVPixelBuffer` respecting its
    /// (possibly padded) plane strides — VideoToolbox's pixel buffer pool
    /// does not promise `stride == width`.
    unsafe fn make_pixel_buffer(&self, nv12: &[u8]) -> Result<CVPixelBufferRef, VideoFileError> {
        let mut pixel_buffer: CVPixelBufferRef = std::ptr::null_mut();
        let status = CVPixelBufferCreate(
            std::ptr::null(),
            self.width as usize,
            self.height as usize,
            kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            std::ptr::null(),
            &mut pixel_buffer,
        );
        if status != 0 || pixel_buffer.is_null() {
            return Err(VideoFileError::with_code("CVPixelBufferCreate", status));
        }
        CVPixelBufferLockBaseAddress(pixel_buffer, 0);
        let width = self.width as usize;
        let height = self.height as usize;
        let y_size = width * height;
        let (y_plane, uv_plane) = nv12.split_at(y_size);

        let y_dst = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0) as *mut u8;
        let y_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0);
        for row in 0..height {
            let src = &y_plane[row * width..(row + 1) * width];
            std::ptr::copy_nonoverlapping(src.as_ptr(), y_dst.add(row * y_stride), src.len());
        }

        let uv_dst = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 1) as *mut u8;
        let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 1);
        let uv_height = height / 2;
        let uv_row_bytes = width; // interleaved U,V pairs: width/2 pairs * 2 bytes
        for row in 0..uv_height {
            let src = &uv_plane[row * uv_row_bytes..(row + 1) * uv_row_bytes];
            std::ptr::copy_nonoverlapping(src.as_ptr(), uv_dst.add(row * uv_stride), src.len());
        }
        CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
        Ok(pixel_buffer)
    }

    pub fn push_frame_nv12(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        unsafe {
            let pixel_buffer = self.make_pixel_buffer(nv12)?;
            let pts = hns_time(pts_100ns);
            let frame_props: CFDictionaryRef = if self.force_keyframe {
                let keys = [kVTEncodeFrameOptionKey_ForceKeyFrame as *const c_void];
                let values = [kCFBooleanTrue as *const c_void];
                CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 1, std::ptr::null(), std::ptr::null())
            } else {
                std::ptr::null()
            };
            self.force_keyframe = false;
            let mut info_flags: VTEncodeInfoFlags = 0;
            let status = VTCompressionSessionEncodeFrame(
                self.session,
                pixel_buffer,
                pts,
                self.frame_duration,
                frame_props,
                std::ptr::null_mut(),
                &mut info_flags,
            );
            if !frame_props.is_null() {
                CFRelease(frame_props as *const c_void);
            }
            CVPixelBufferRelease(pixel_buffer);
            if status != 0 {
                return Err(VideoFileError::with_code("VTCompressionSessionEncodeFrame", status));
            }
            let complete_status = VTCompressionSessionCompleteFrames(self.session, kCMTimePositiveInfinity);
            if complete_status != 0 {
                return Err(VideoFileError::with_code("VTCompressionSessionCompleteFrames", complete_status));
            }
        }
        let mut guard = self
            .shared
            .lock()
            .map_err(|_| VideoFileError::new("stream encoder output queue poisoned"))?;
        Ok(guard
            .packets
            .drain(..)
            .map(|(data, pts_100ns, is_key)| EncodedPacket { data, pts_100ns, is_key })
            .collect())
    }

    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }
}

// SAFETY: `VTCompressionSessionRef` is an opaque, atomically-refcounted
// Core Foundation-style object with no thread-affinity requirement (unlike
// e.g. UI objects) — VideoToolbox only requires the CALLER not invoke it
// concurrently from multiple threads, which `AppleStreamEncoder`'s `&mut
// self` API already enforces (exclusive access). Moving the whole struct
// (ownership, not concurrent access) to a different thread between calls is
// sound; `makepad-asset-ai`'s realtime session holds this behind a `Mutex`
// precisely so it is never accessed from two threads at once.
unsafe impl Send for AppleStreamEncoder {}

impl Drop for AppleStreamEncoder {
    fn drop(&mut self) {
        unsafe {
            VTCompressionSessionCompleteFrames(self.session, kCMTimePositiveInfinity);
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session as *const c_void);
        }
    }
}
