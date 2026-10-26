//! macOS backend for the low-latency stream decoder: `VTDecompressionSession`
//! driven directly from Annex-B access units. H.264 and HEVC.
//!
//! The decoder (re)builds its `CMFormatDescription`/`VTDecompressionSession`
//! whenever it sees SPS/PPS NALs (every keyframe packet carries them, per
//! [`crate::stream_encoder`]'s contract) — this both bootstraps the very
//! first session and transparently handles a mid-stream parameter change
//! (e.g. the encoder side resizing). Driven synchronously, mirroring the
//! encoder: `push_packet` waits for the decode callback before returning.

use crate::annex_b;
use crate::stream_decoder::{DecodedFrame, DecodedPicture};
use crate::stream_encoder::StreamVideoCodec;
use crate::{VideoColor, VideoFileError};
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

struct DecoderShared {
    /// Frames stay the decoder's IOSurface-backed pixel buffers (no CPU copy).
    gpu: bool,
    frames: Vec<DecodedFrame>,
    /// A picture the decoder reported as failed (its status): the caller hears it (and asks for a
    /// key frame) instead of decoding on from a damaged reference.
    failed: Option<OSStatus>,
    /// The colour the stream's parameter sets declare (its SPS VUI): pictures without colour
    /// attachments of their own are this.
    stream_color: VideoColor,
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferGetIOSurface(buffer: CVPixelBufferRef) -> *mut c_void;
}

/// kCVPixelFormatType_444YpCbCr8BiPlanarVideoRange ('444v'): luma, then CbCr at every pixel.
const PIXEL_444V: u32 = u32::from_be_bytes(*b"444v");

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTSessionCopyProperty(session: *mut c_void, key: *const c_void, allocator: *const c_void, value_out: *mut *const c_void) -> OSStatus;
    static kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder: *const c_void;
}

/// A decoded pixel buffer held for a `DecodedPicture`: back to the decoder's pool when released.
struct HeldPixelBuffer(CVPixelBufferRef);

// SAFETY: a CVPixelBuffer is a thread-safe refcounted CoreVideo object; this only releases it.
unsafe impl Send for HeldPixelBuffer {}
unsafe impl Sync for HeldPixelBuffer {}

impl Drop for HeldPixelBuffer {
    fn drop(&mut self) {
        unsafe { CVPixelBufferRelease(self.0) };
    }
}

unsafe extern "C" fn decode_output_callback(
    decompression_output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: OSStatus,
    _info_flags: VTDecodeInfoFlags,
    image_buffer: CVImageBufferRef,
    presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    if status != 0 {
        let shared = &*(decompression_output_ref_con as *const Mutex<DecoderShared>);
        if let Ok(mut guard) = shared.lock() {
            guard.failed = Some(status);
        }
        return;
    }
    if image_buffer.is_null() {
        return;
    }
    let width = CVPixelBufferGetWidth(image_buffer) as u32;
    let height = CVPixelBufferGetHeight(image_buffer) as u32;
    if width == 0 || height == 0 {
        return;
    }
    let pts_100ns = cmtime_to_hns(presentation_time_stamp);
    let shared = &*(decompression_output_ref_con as *const Mutex<DecoderShared>);
    let Ok(mut guard) = shared.lock() else { return };
    let color = crate::apple_decoder::buffer_color(image_buffer, guard.stream_color, width, height);
    if guard.gpu {
        let surface = CVPixelBufferGetIOSurface(image_buffer);
        if !surface.is_null() {
            let handle = IOSurfaceGetID(surface) as u64;
            let held = HeldPixelBuffer(CVPixelBufferRetain(image_buffer));
            let full_chroma = CVPixelBufferGetPixelFormatType(image_buffer) == PIXEL_444V;
            let gpu = Some(DecodedPicture { handle, full_chroma, _keep: std::sync::Arc::new(held) });
            guard.frames.push(DecodedFrame { width, height, nv12: Vec::new(), pts_100ns, gpu, color });
            return;
        }
    }
    drop(guard);
    CVPixelBufferLockBaseAddress(image_buffer, 1); // kCVPixelBufferLock_ReadOnly
    let mut nv12 = vec![0u8; crate::nv12::nv12_frame_size(width, height)];
    let y_size = width as usize * height as usize;
    {
        let (y_dst, uv_dst) = nv12.split_at_mut(y_size);
        let y_src = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 0) as *const u8;
        let y_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 0);
        for row in 0..height as usize {
            let dst = &mut y_dst[row * width as usize..(row + 1) * width as usize];
            std::ptr::copy_nonoverlapping(y_src.add(row * y_stride), dst.as_mut_ptr(), dst.len());
        }
        let uv_src = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 1) as *const u8;
        let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 1);
        let uv_height = height as usize / 2;
        let uv_row_bytes = width as usize;
        for row in 0..uv_height {
            let dst = &mut uv_dst[row * uv_row_bytes..(row + 1) * uv_row_bytes];
            std::ptr::copy_nonoverlapping(uv_src.add(row * uv_stride), dst.as_mut_ptr(), dst.len());
        }
    }
    CVPixelBufferUnlockBaseAddress(image_buffer, 1);
    if let Ok(mut guard) = shared.lock() {
        guard.frames.push(DecodedFrame { width, height, nv12, pts_100ns, gpu: None, color });
    }
}

pub struct AppleStreamDecoder {
    session: VTDecompressionSessionRef,
    format_desc: CMFormatDescriptionRef,
    shared: Box<Mutex<DecoderShared>>,
    gpu: bool,
    codec: StreamVideoCodec,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    /// HEVC only; H.264 has no VPS.
    vps: Option<Vec<u8>>,
}

impl AppleStreamDecoder {
    /// `gpu`: frames stay IOSurface-backed pixel buffers (`DecodedFrame::gpu`), no CPU copy.
    pub fn new(codec: StreamVideoCodec, gpu: bool) -> Result<Self, VideoFileError> {
        Ok(Self {
            session: std::ptr::null_mut(),
            format_desc: std::ptr::null_mut(),
            gpu,
            shared: Box::new(Mutex::new(DecoderShared { gpu, frames: Vec::new(), failed: None, stream_color: VideoColor::BT709 })),
            codec,
            sps: None,
            pps: None,
            vps: None,
        })
    }

    unsafe fn recreate_session(&mut self) -> Result<(), VideoFileError> {
        let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
            return Ok(());
        };
        if matches!(self.codec, StreamVideoCodec::Hevc) && self.vps.is_none() {
            return Ok(());
        }
        if !self.session.is_null() {
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session as *const c_void);
            self.session = std::ptr::null_mut();
        }
        if !self.format_desc.is_null() {
            CFRelease(self.format_desc as *const c_void);
            self.format_desc = std::ptr::null_mut();
        }
        let mut format_desc: CMFormatDescriptionRef = std::ptr::null_mut();
        let status = match self.codec {
            StreamVideoCodec::H264 => {
                let ptrs: [*const u8; 2] = [sps.as_ptr(), pps.as_ptr()];
                let sizes: [usize; 2] = [sps.len(), pps.len()];
                CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    std::ptr::null(),
                    2,
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    &mut format_desc,
                )
            }
            StreamVideoCodec::Hevc => {
                let vps = self.vps.as_ref().unwrap();
                let ptrs: [*const u8; 3] = [vps.as_ptr(), sps.as_ptr(), pps.as_ptr()];
                let sizes: [usize; 3] = [vps.len(), sps.len(), pps.len()];
                CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    std::ptr::null(),
                    3,
                    ptrs.as_ptr(),
                    sizes.as_ptr(),
                    4,
                    std::ptr::null(),
                    &mut format_desc,
                )
            }
        };
        if status != 0 || format_desc.is_null() {
            return Err(VideoFileError::with_code(
                "CMVideoFormatDescriptionCreateFromParameterSets",
                status,
            ));
        }
        self.format_desc = format_desc;
        let dims = CMVideoFormatDescriptionGetDimensions(format_desc);
        let stream_color = crate::apple_decoder::format_color(format_desc, dims.width as u32, dims.height as u32);
        if let Ok(mut guard) = self.shared.lock() {
            guard.stream_color = stream_color;
        }

        let callback_record = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(decode_output_callback),
            decompressionOutputRefCon: self.shared.as_ref() as *const Mutex<DecoderShared> as *mut c_void,
        };
        let mut session: VTDecompressionSessionRef = std::ptr::null_mut();
        // GPU output: 420v pixel buffers on global IOSurfaces (the HAL imports them by id) that
        // Metal can sample, plane by plane
        // a 4:4:4 stream (H.264 High 4:4:4 Predictive, profile_idc 244; H.265 chroma_format_idc 3)
        // stays 4:4:4 on the GPU; the CPU
        // form is always tight NV12
        let full_chroma = match self.codec {
            StreamVideoCodec::H264 => sps.get(1) == Some(&244),
            StreamVideoCodec::Hevc => crate::hevc_config::sps_chroma_format_idc(sps) == Some(3),
        };
        let number = |v: u32| -> ObjcId { msg_send![class!(NSNumber), numberWithUnsignedInt: v] };
        let destination: ObjcId = if self.gpu {
            let yes: ObjcId = msg_send![class!(NSNumber), numberWithBool: YES];
            let global_key: ObjcId = msg_send![class!(NSString), stringWithUTF8String: c"IOSurfaceIsGlobal".as_ptr()];
            let surface_props: ObjcId = msg_send![class!(NSDictionary), dictionaryWithObject: yes forKey: global_key];
            let keys = [kCVPixelBufferPixelFormatTypeKey as ObjcId, kCVPixelBufferIOSurfacePropertiesKey as ObjcId, kCVPixelBufferMetalCompatibilityKey as ObjcId];
            let values = [number(if full_chroma { PIXEL_444V } else { kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange }), surface_props, yes];
            msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr() forKeys: keys.as_ptr() count: keys.len()]
        } else {
            let keys = [kCVPixelBufferPixelFormatTypeKey as ObjcId];
            let values = [number(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)];
            msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr() forKeys: keys.as_ptr() count: keys.len()]
        };
        let create_status = VTDecompressionSessionCreate(
            std::ptr::null(),
            format_desc,
            std::ptr::null(),
            destination as *const c_void,
            &callback_record,
            &mut session,
        );
        if create_status != 0 || session.is_null() {
            return Err(VideoFileError::with_code("VTDecompressionSessionCreate", create_status));
        }
        self.session = session;
        Ok(())
    }

    pub fn push_packet(&mut self, annex_b_data: &[u8], pts_100ns: i64) -> Result<Vec<DecodedFrame>, VideoFileError> {
        let nals = annex_b::split_annex_b(annex_b_data);
        let mut vcl_nals: Vec<&[u8]> = Vec::new();
        let mut params_updated = false;
        for nal in &nals {
            match self.codec {
                StreamVideoCodec::H264 => match annex_b::nal_unit_type(nal) {
                    annex_b::NAL_TYPE_SPS => {
                        self.sps = Some(nal.to_vec());
                        params_updated = true;
                    }
                    annex_b::NAL_TYPE_PPS => {
                        self.pps = Some(nal.to_vec());
                        params_updated = true;
                    }
                    _ => vcl_nals.push(nal),
                },
                StreamVideoCodec::Hevc => match annex_b::hevc_nal_unit_type(nal) {
                    annex_b::HEVC_NAL_TYPE_VPS => {
                        self.vps = Some(nal.to_vec());
                        params_updated = true;
                    }
                    annex_b::HEVC_NAL_TYPE_SPS => {
                        self.sps = Some(nal.to_vec());
                        params_updated = true;
                    }
                    annex_b::HEVC_NAL_TYPE_PPS => {
                        self.pps = Some(nal.to_vec());
                        params_updated = true;
                    }
                    _ => vcl_nals.push(nal),
                },
            }
        }
        if params_updated {
            unsafe { self.recreate_session()? };
        }
        if vcl_nals.is_empty() {
            return Ok(Vec::new());
        }
        if self.session.is_null() {
            return Err(VideoFileError::new(
                "apple stream decoder: no SPS/PPS seen yet — cannot decode a VCL NAL",
            ));
        }
        let avcc = annex_b::annex_b_to_avcc(&vcl_nals);
        unsafe {
            let mut block: CMBlockBufferRef = std::ptr::null_mut();
            let status = CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null(),
                std::ptr::null_mut(),
                avcc.len(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                avcc.len(),
                0,
                &mut block,
            );
            if status != 0 || block.is_null() {
                return Err(VideoFileError::with_code("CMBlockBufferCreateWithMemoryBlock", status));
            }
            let assure_status = CMBlockBufferAssureBlockMemory(block);
            if assure_status != 0 {
                CFRelease(block as *const c_void);
                return Err(VideoFileError::with_code("CMBlockBufferAssureBlockMemory", assure_status));
            }
            let replace_status = CMBlockBufferReplaceDataBytes(avcc.as_ptr() as *const c_void, block, 0, avcc.len());
            if replace_status != 0 {
                CFRelease(block as *const c_void);
                return Err(VideoFileError::with_code("CMBlockBufferReplaceDataBytes", replace_status));
            }

            let pts = hns_time(pts_100ns);
            let timing = CMSampleTimingInfo {
                duration: kCMTimeInvalid,
                presentationTimeStamp: pts,
                decodeTimeStamp: kCMTimeInvalid,
            };
            let sample_size: usize = avcc.len();
            let mut sample: CMSampleBufferRef = std::ptr::null_mut();
            let sb_status = CMSampleBufferCreateReady(
                std::ptr::null(),
                block,
                self.format_desc,
                1,
                1,
                &timing,
                1,
                &sample_size,
                &mut sample,
            );
            CFRelease(block as *const c_void);
            if sb_status != 0 || sample.is_null() {
                return Err(VideoFileError::with_code("CMSampleBufferCreateReady", sb_status));
            }

            let mut info_flags: VTDecodeInfoFlags = 0;
            let decode_status =
                VTDecompressionSessionDecodeFrame(self.session, sample, 0, std::ptr::null_mut(), &mut info_flags);
            CFRelease(sample as *const c_void);
            if decode_status != 0 {
                return Err(VideoFileError::with_code("VTDecompressionSessionDecodeFrame", decode_status));
            }
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
        }
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        let mut guard = self
            .shared
            .lock()
            .map_err(|_| VideoFileError::new("stream decoder output queue poisoned"))?;
        if let Some(status) = guard.failed.take() {
            guard.frames.clear();
            return Err(VideoFileError::with_code("VTDecompressionSession decode (picture)", status));
        }
        Ok(guard
            .frames
            .drain(..)
            .collect())
    }

    /// Whether the session decodes on the hardware decoder (None before a session exists).
    pub fn hardware(&self) -> Option<bool> {
        if self.session.is_null() {
            return None;
        }
        unsafe {
            let mut value: *const c_void = std::ptr::null();
            if VTSessionCopyProperty(self.session as *mut c_void, kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder, std::ptr::null(), &mut value) != 0 || value.is_null() {
                return None;
            }
            let on = value == kCFBooleanTrue as *const c_void;
            CFRelease(value);
            Some(on)
        }
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        unsafe {
            if !self.session.is_null() {
                VTDecompressionSessionWaitForAsynchronousFrames(self.session);
            }
        }
        self.drain()
    }
}

// SAFETY: see `AppleStreamEncoder`'s identical `Send` impl in
// apple_stream_encoder.rs — same reasoning (opaque refcounted VideoToolbox
// session object, no thread affinity, caller-enforced exclusive access).
unsafe impl Send for AppleStreamDecoder {}

impl Drop for AppleStreamDecoder {
    fn drop(&mut self) {
        unsafe {
            if !self.session.is_null() {
                VTDecompressionSessionInvalidate(self.session);
                CFRelease(self.session as *const c_void);
            }
            if !self.format_desc.is_null() {
                CFRelease(self.format_desc as *const c_void);
            }
        }
    }
}
