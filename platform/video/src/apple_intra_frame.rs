//! One still picture through the hardware encoder, and nothing else.
//!
//! `AVAssetWriter` is the general answer — a container, a track, a queue, an
//! asynchronous finalize — and for a single frame almost all of that is
//! ceremony. Here the compression session is driven directly: push one pixel
//! buffer, take the access unit and the codec's own configuration atom back
//! out, and let [`crate::mp4_single_frame`] write the boxes.
//!
//! Sessions are bound to one frame size, so a cache of them helps only when
//! sizes repeat — which in a picture corpus they sometimes do (one scanner,
//! one plate size, a hundred pictures). The cache is small and per-thread;
//! missing it costs a session create, which is a fraction of what standing up
//! a writer used to.

use crate::mp4_single_frame::{write_single_frame_mp4, IntraSample};
use crate::{VideoFileError, VideoFileCodec};
use makepad_apple_sys::*;
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Mutex;

/// What one encode produced.
struct Captured {
    /// Length-prefixed NAL units: an mp4 sample as it stands.
    data: Vec<u8>,
    /// `hvcC` / `avcC`, verbatim from the format description.
    config: Vec<u8>,
}

#[derive(Default)]
struct EncodeShared {
    captured: Option<Captured>,
}

/// Copy a `CFData` out to a `Vec`.
unsafe fn cfdata_bytes(data: CFDataRef) -> Option<Vec<u8>> {
    if data.is_null() {
        return None;
    }
    let len = CFDataGetLength(data);
    let ptr = CFDataGetBytePtr(data);
    if ptr.is_null() || len <= 0 {
        return None;
    }
    Some(std::slice::from_raw_parts(ptr, len as usize).to_vec())
}

/// The codec configuration atom the encoder hung off its format description.
/// Taking it whole is the point: profile, tier, level and every parameter set
/// are the encoder's own, not something re-derived by parsing the bitstream.
unsafe fn config_atom(format_desc: CMFormatDescriptionRef, name: &str) -> Option<Vec<u8>> {
    if format_desc.is_null() {
        return None;
    }
    let atoms = CMFormatDescriptionGetExtension(
        format_desc,
        kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
    ) as CFDictionaryRef;
    if atoms.is_null() {
        return None;
    }
    let key = CFStringCreateWithBytes(
        std::ptr::null(),
        name.as_ptr(),
        name.len() as CFIndex,
        kCFStringEncodingUTF8,
        0,
    );
    if key.is_null() {
        return None;
    }
    let value = CFDictionaryGetValue(atoms, key as *const c_void) as CFDataRef;
    let out = cfdata_bytes(value);
    CFRelease(key as *const c_void);
    out
}

unsafe extern "C" fn intra_output_callback(
    output_callback_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: OSStatus,
    _info_flags: VTEncodeInfoFlags,
    sample_buffer: CMSampleBufferRef,
) {
    if status != 0 || sample_buffer.is_null() {
        return;
    }
    let shared = &*(output_callback_ref_con as *const Mutex<EncodeShared>);
    let block_buffer = CMSampleBufferGetDataBuffer(sample_buffer);
    if block_buffer.is_null() {
        return;
    }
    let len = CMBlockBufferGetDataLength(block_buffer);
    if len <= 0 {
        return;
    }
    let mut data = vec![0u8; len as usize];
    if CMBlockBufferCopyDataBytes(block_buffer, 0, len, data.as_mut_ptr() as *mut c_void) != 0 {
        return;
    }
    let format_desc = CMSampleBufferGetFormatDescription(sample_buffer);
    // Which atom to ask for follows from the codec the session was made with;
    // try HEVC first and fall back, rather than threading the codec in here.
    let config = config_atom(format_desc, "hvcC").or_else(|| config_atom(format_desc, "avcC"));
    let Some(config) = config else { return };
    if let Ok(mut guard) = shared.lock() {
        guard.captured = Some(Captured { data, config });
    }
}

/// A compression session and the size it is bound to.
struct Session {
    session: VTCompressionSessionRef,
    shared: Box<Mutex<EncodeShared>>,
    width: u32,
    height: u32,
    codec: VideoFileCodec,
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session as *const c_void);
        }
    }
}

unsafe fn set_i32_property(session: VTCompressionSessionRef, key: CFStringRef, value: i32) {
    let number = CFNumberCreate(std::ptr::null(), kCFNumberSInt32Type, &value as *const i32 as *const c_void);
    if !number.is_null() {
        VTSessionSetProperty(session as *mut c_void, key, number as *const c_void);
        CFRelease(number as *const c_void);
    }
}

unsafe fn set_bool_property(session: VTCompressionSessionRef, key: CFStringRef, value: bool) {
    let v = if value { kCFBooleanTrue } else { kCFBooleanFalse };
    VTSessionSetProperty(session as *mut c_void, key, v as *const c_void);
}

impl Session {
    fn new(width: u32, height: u32, codec: VideoFileCodec) -> Result<Session, VideoFileError> {
        let codec_type = match codec {
            VideoFileCodec::H265 => kCMVideoCodecType_HEVC,
            _ => kCMVideoCodecType_H264,
        };
        let shared = Box::new(Mutex::new(EncodeShared::default()));
        let refcon = shared.as_ref() as *const Mutex<EncodeShared> as *mut c_void;
        let mut session: VTCompressionSessionRef = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSessionCreate(
                std::ptr::null(),
                width,
                height,
                codec_type,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                Some(intra_output_callback),
                refcon,
                &mut session,
            )
        };
        if status != 0 || session.is_null() {
            return Err(VideoFileError::with_code("VTCompressionSessionCreate", status));
        }
        unsafe {
            // Offline work: let the encoder take its time and spend it well.
            set_bool_property(session, kVTCompressionPropertyKey_RealTime, true);
            set_bool_property(session, kVTCompressionPropertyKey_AllowFrameReordering, false);
            // Every frame a keyframe — there is only ever one.
            set_i32_property(session, kVTCompressionPropertyKey_MaxKeyFrameInterval, 1);
            if matches!(codec, VideoFileCodec::H265) {
                VTSessionSetProperty(
                    session as *mut c_void,
                    kVTCompressionPropertyKey_ProfileLevel,
                    kVTProfileLevel_HEVC_Main_AutoLevel as *const c_void,
                );
            }
        }
        Ok(Session { session, shared, width, height, codec })
    }

    /// Encode one NV12 frame and return the access unit with its config atom.
    fn encode(&mut self, nv12: &[u8], bitrate_bps: u32) -> Result<Captured, VideoFileError> {
        let (width, height) = (self.width as usize, self.height as usize);
        let uv_height = height.div_ceil(2);
        if nv12.len() < width * (height + uv_height) {
            return Err(VideoFileError::new("nv12 buffer is short for the frame size"));
        }
        if let Ok(mut guard) = self.shared.lock() {
            guard.captured = None;
        }
        unsafe {
            set_i32_property(
                self.session,
                kVTCompressionPropertyKey_AverageBitRate,
                bitrate_bps.min(i32::MAX as u32) as i32,
            );
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
            // The pool does not promise stride == width, so copy row by row.
            for row in 0..height {
                std::ptr::copy_nonoverlapping(nv12.as_ptr().add(row * width), y_ptr.add(row * y_stride), width);
            }
            for row in 0..uv_height {
                std::ptr::copy_nonoverlapping(
                    nv12.as_ptr().add((height + row) * width),
                    uv_ptr.add(row * uv_stride),
                    width,
                );
            }
            CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);

            let pts = CMTime { value: 0, timescale: 600, flags: kCMTimeFlags_Valid, epoch: 0 };
            let duration = CMTime { value: 20, timescale: 600, flags: kCMTimeFlags_Valid, epoch: 0 };
            let encode_status = VTCompressionSessionEncodeFrame(
                self.session,
                pixel_buffer,
                pts,
                duration,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            CVPixelBufferRelease(pixel_buffer);
            if encode_status != 0 {
                return Err(VideoFileError::with_code("VTCompressionSessionEncodeFrame", encode_status));
            }
            let complete_status = VTCompressionSessionCompleteFrames(
                self.session,
                CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 },
            );
            if complete_status != 0 {
                return Err(VideoFileError::with_code("VTCompressionSessionCompleteFrames", complete_status));
            }
        }
        self.shared
            .lock()
            .ok()
            .and_then(|mut g| g.captured.take())
            .ok_or_else(|| VideoFileError::new("encoder produced no frame"))
    }
}

thread_local! {
    /// Sessions are not shared between threads: a session is stateful and the
    /// callback writes into storage owned beside it. Each encode thread keeps
    /// its own few.
    static SESSIONS: RefCell<Vec<Session>> = const { RefCell::new(Vec::new()) };
}

/// How many sizes one thread keeps sessions for. Small on purpose: each live
/// session is a claim on the one hardware encoder.
const SESSION_CACHE: usize = 2;

/// Encode one NV12 frame and return a complete single-frame mp4.
pub fn encode_intra_frame_mp4(
    nv12: &[u8],
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
    codec: VideoFileCodec,
) -> Result<Vec<u8>, VideoFileError> {
    let captured = SESSIONS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let found = cache
            .iter()
            .position(|s| s.width == width && s.height == height && s.codec == codec);
        let mut session = match found {
            Some(index) => cache.remove(index),
            None => Session::new(width, height, codec)?,
        };
        let out = session.encode(nv12, bitrate_bps);
        // Most recently used at the front; a session that failed is dropped
        // rather than kept, since its state is no longer something we know.
        if out.is_ok() {
            cache.insert(0, session);
            cache.truncate(SESSION_CACHE);
        }
        out
    })?;

    let (sample_entry, config_atom) = match codec {
        VideoFileCodec::H265 => (*b"hvc1", *b"hvcC"),
        _ => (*b"avc1", *b"avcC"),
    };
    Ok(write_single_frame_mp4(&IntraSample {
        data: &captured.data,
        config: &captured.config,
        sample_entry,
        config_atom,
        width,
        height,
        fps,
    }))
}
