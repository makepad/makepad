//! macOS backend for the low-latency stream decoder: `VTDecompressionSession`
//! driven directly from Annex-B access units.
//!
//! The decoder (re)builds its `CMFormatDescription`/`VTDecompressionSession`
//! whenever it sees SPS/PPS NALs (every keyframe packet carries them, per
//! [`crate::stream_encoder`]'s contract) — this both bootstraps the very
//! first session and transparently handles a mid-stream parameter change
//! (e.g. the encoder side resizing). Driven synchronously, mirroring the
//! encoder: `push_packet` waits for the decode callback before returning.

use crate::annex_b;
use crate::stream_decoder::DecodedFrame;
use crate::stream_encoder::StreamVideoCodec;
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

struct DecoderShared {
    frames: Vec<(u32, u32, Vec<u8>, i64)>,
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
    if status != 0 || image_buffer.is_null() {
        return;
    }
    let width = CVPixelBufferGetWidth(image_buffer) as u32;
    let height = CVPixelBufferGetHeight(image_buffer) as u32;
    if width == 0 || height == 0 {
        return;
    }
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
    let pts_100ns = cmtime_to_hns(presentation_time_stamp);
    let shared = &*(decompression_output_ref_con as *const Mutex<DecoderShared>);
    if let Ok(mut guard) = shared.lock() {
        guard.frames.push((width, height, nv12, pts_100ns));
    }
}

pub struct AppleStreamDecoder {
    session: VTDecompressionSessionRef,
    format_desc: CMFormatDescriptionRef,
    shared: Box<Mutex<DecoderShared>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl AppleStreamDecoder {
    pub fn new(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        if !matches!(codec, StreamVideoCodec::H264) {
            return Err(VideoFileError::new("apple stream decoder: only H264 is implemented"));
        }
        Ok(Self {
            session: std::ptr::null_mut(),
            format_desc: std::ptr::null_mut(),
            shared: Box::new(Mutex::new(DecoderShared { frames: Vec::new() })),
            sps: None,
            pps: None,
        })
    }

    unsafe fn recreate_session(&mut self) -> Result<(), VideoFileError> {
        let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
            return Ok(());
        };
        if !self.session.is_null() {
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session as *const c_void);
            self.session = std::ptr::null_mut();
        }
        if !self.format_desc.is_null() {
            CFRelease(self.format_desc as *const c_void);
            self.format_desc = std::ptr::null_mut();
        }
        let ptrs: [*const u8; 2] = [sps.as_ptr(), pps.as_ptr()];
        let sizes: [usize; 2] = [sps.len(), pps.len()];
        let mut format_desc: CMFormatDescriptionRef = std::ptr::null_mut();
        let status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
            std::ptr::null(),
            2,
            ptrs.as_ptr(),
            sizes.as_ptr(),
            4,
            &mut format_desc,
        );
        if status != 0 || format_desc.is_null() {
            return Err(VideoFileError::with_code(
                "CMVideoFormatDescriptionCreateFromH264ParameterSets",
                status,
            ));
        }
        self.format_desc = format_desc;

        let callback_record = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(decode_output_callback),
            decompressionOutputRefCon: self.shared.as_ref() as *const Mutex<DecoderShared> as *mut c_void,
        };
        let mut session: VTDecompressionSessionRef = std::ptr::null_mut();
        let create_status = VTDecompressionSessionCreate(
            std::ptr::null(),
            format_desc,
            std::ptr::null(),
            std::ptr::null(),
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
            match annex_b::nal_unit_type(nal) {
                annex_b::NAL_TYPE_SPS => {
                    self.sps = Some(nal.to_vec());
                    params_updated = true;
                }
                annex_b::NAL_TYPE_PPS => {
                    self.pps = Some(nal.to_vec());
                    params_updated = true;
                }
                _ => vcl_nals.push(nal),
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
        Ok(guard
            .frames
            .drain(..)
            .map(|(width, height, nv12, pts_100ns)| DecodedFrame { width, height, nv12, pts_100ns })
            .collect())
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
