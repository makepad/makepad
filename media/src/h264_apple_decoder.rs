use {
    crate::h264_packets,
    makepad_platform::{
        os::apple::apple_sys::*,
        MseDecodedFrame, VideoFrameDecoder,
        video_decode::yuv::{YuvColorMatrix, YuvLayout, YuvPlaneData},
    },
    std::{
        collections::VecDeque,
        ffi::c_void,
        sync::{Arc, Mutex},
    },
};

struct DecoderQueue {
    frames: VecDeque<MseDecodedFrame>,
}

pub struct AppleH264Decoder {
    session: VTDecompressionSessionRef,
    format_desc: CMFormatDescriptionRef,
    queue: Arc<Mutex<DecoderQueue>>,
}

unsafe impl Send for AppleH264Decoder {}

impl AppleH264Decoder {
    pub fn new(sps_pps_annexb: &[u8]) -> Result<Self, String> {
        let (sps_list, pps_list) = h264_packets::annexb_to_sps_pps(sps_pps_annexb);
        if sps_list.is_empty() || pps_list.is_empty() {
            return Err("no SPS/PPS found in Annex B data".into());
        }

        unsafe {
            // Build parameter set arrays
            let mut param_ptrs: Vec<*const u8> = Vec::new();
            let mut param_sizes: Vec<usize> = Vec::new();
            for sps in &sps_list {
                param_ptrs.push(sps.as_ptr());
                param_sizes.push(sps.len());
            }
            for pps in &pps_list {
                param_ptrs.push(pps.as_ptr());
                param_sizes.push(pps.len());
            }

            let mut format_desc: CMFormatDescriptionRef = std::ptr::null_mut();
            let status = CMVideoFormatDescriptionCreateFromH264ParameterSets(
                std::ptr::null(),
                param_ptrs.len(),
                param_ptrs.as_ptr(),
                param_sizes.as_ptr(),
                4, // NAL unit header length
                &mut format_desc,
            );
            if status != 0 || format_desc.is_null() {
                return Err(format!("CMVideoFormatDescriptionCreateFromH264ParameterSets failed: {}", status));
            }

            let queue = Arc::new(Mutex::new(DecoderQueue {
                frames: VecDeque::new(),
            }));

            let callback_ctx = Box::into_raw(Box::new(CallbackContext {
                queue: queue.clone(),
            }));

            let callback_record = VTDecompressionOutputCallbackRecord {
                decompressionOutputCallback: Some(decode_output_callback),
                decompressionOutputRefCon: callback_ctx as *mut c_void,
            };

            let mut session: VTDecompressionSessionRef = std::ptr::null_mut();
            let status = VTDecompressionSessionCreate(
                std::ptr::null(),
                format_desc,
                std::ptr::null(),  // decoder specification
                std::ptr::null(),  // destination image buffer attributes (default NV12)
                &callback_record,
                &mut session,
            );
            if status != 0 || session.is_null() {
                CFRelease(format_desc as *const c_void);
                let _ = Box::from_raw(callback_ctx);
                return Err(format!("VTDecompressionSessionCreate failed: {}", status));
            }

            Ok(AppleH264Decoder {
                session,
                format_desc,
                queue,
            })
        }
    }
}

struct CallbackContext {
    queue: Arc<Mutex<DecoderQueue>>,
}

unsafe extern "C" fn decode_output_callback(
    refcon: *mut c_void,
    _source_frame_refcon: *mut c_void,
    status: OSStatus,
    _info_flags: VTDecodeInfoFlags,
    image_buffer: CVImageBufferRef,
    presentation_timestamp: CMTime,
    _presentation_duration: CMTime,
) {
    if status != 0 || image_buffer.is_null() {
        return;
    }

    let ctx = &*(refcon as *const CallbackContext);

    let pts_ms = if presentation_timestamp.timescale > 0 {
        (presentation_timestamp.value.max(0) as u64)
            .saturating_mul(1000)
            .saturating_div(presentation_timestamp.timescale as u64)
    } else {
        0
    };

    CVPixelBufferLockBaseAddress(image_buffer, 1); // read-only lock

    let pixel_format = CVPixelBufferGetPixelFormatType(image_buffer);
    let w = CVPixelBufferGetWidth(image_buffer) as u32;
    let h = CVPixelBufferGetHeight(image_buffer) as u32;

    let frame = if pixel_format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        || pixel_format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
    {
        // NV12: 2 planes (Y + interleaved UV) → convert to I420
        extract_nv12_to_i420(image_buffer, w, h, pts_ms)
    } else if pixel_format == kCVPixelFormatType_420YpCbCr8Planar {
        // I420: 3 planes
        extract_i420(image_buffer, w, h, pts_ms)
    } else {
        // Unsupported pixel format
        None
    };

    CVPixelBufferUnlockBaseAddress(image_buffer, 1);

    if let Some(frame) = frame {
        let mut q = ctx.queue.lock().unwrap();
        q.frames.push_back(frame);
    }
}

unsafe fn extract_nv12_to_i420(
    image_buffer: CVImageBufferRef,
    w: u32,
    h: u32,
    pts_ms: u64,
) -> Option<MseDecodedFrame> {
    let plane_count = CVPixelBufferGetPlaneCount(image_buffer);
    if plane_count < 2 {
        return None;
    }

    let y_base = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 0) as *const u8;
    let uv_base = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 1) as *const u8;
    if y_base.is_null() || uv_base.is_null() {
        return None;
    }

    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 0);
    let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 1);
    let uv_w = (w as usize + 1) / 2;
    let uv_h = (h as usize + 1) / 2;

    // Copy Y plane
    let mut y = Vec::with_capacity(w as usize * h as usize);
    for row in 0..h as usize {
        let src = std::slice::from_raw_parts(y_base.add(row * y_stride), w as usize);
        y.extend_from_slice(src);
    }

    // Deinterleave NV12 UV plane into separate U and V
    let mut u_plane = Vec::with_capacity(uv_w * uv_h);
    let mut v_plane = Vec::with_capacity(uv_w * uv_h);
    for row in 0..uv_h {
        let src = std::slice::from_raw_parts(uv_base.add(row * uv_stride), uv_w * 2);
        for col in 0..uv_w {
            u_plane.push(src[col * 2]);
            v_plane.push(src[col * 2 + 1]);
        }
    }

    Some(MseDecodedFrame {
        track_id: 0,
        pts_ms,
        yuv: YuvPlaneData {
            y,
            u: u_plane,
            v: v_plane,
            width: w,
            height: h,
            layout: YuvLayout::I420,
            matrix: YuvColorMatrix::BT709,
        },
    })
}

unsafe fn extract_i420(
    image_buffer: CVImageBufferRef,
    w: u32,
    h: u32,
    pts_ms: u64,
) -> Option<MseDecodedFrame> {
    let plane_count = CVPixelBufferGetPlaneCount(image_buffer);
    if plane_count < 3 {
        return None;
    }

    let y_base = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 0) as *const u8;
    let u_base = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 1) as *const u8;
    let v_base = CVPixelBufferGetBaseAddressOfPlane(image_buffer, 2) as *const u8;
    if y_base.is_null() || u_base.is_null() || v_base.is_null() {
        return None;
    }

    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 0);
    let u_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 1);
    let v_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer, 2);
    let uv_w = (w as usize + 1) / 2;
    let uv_h = (h as usize + 1) / 2;

    let mut y = Vec::with_capacity(w as usize * h as usize);
    for row in 0..h as usize {
        let src = std::slice::from_raw_parts(y_base.add(row * y_stride), w as usize);
        y.extend_from_slice(src);
    }

    let mut u_plane = Vec::with_capacity(uv_w * uv_h);
    for row in 0..uv_h {
        let src = std::slice::from_raw_parts(u_base.add(row * u_stride), uv_w);
        u_plane.extend_from_slice(src);
    }

    let mut v_plane = Vec::with_capacity(uv_w * uv_h);
    for row in 0..uv_h {
        let src = std::slice::from_raw_parts(v_base.add(row * v_stride), uv_w);
        v_plane.extend_from_slice(src);
    }

    Some(MseDecodedFrame {
        track_id: 0,
        pts_ms,
        yuv: YuvPlaneData {
            y,
            u: u_plane,
            v: v_plane,
            width: w,
            height: h,
            layout: YuvLayout::I420,
            matrix: YuvColorMatrix::BT709,
        },
    })
}

impl VideoFrameDecoder for AppleH264Decoder {
    fn push_data(&mut self, data: &[u8], pts_ms: u64) -> Result<(), String> {
        // Convert Annex B NAL units to AVCC format (4-byte length prefix)
        let nals = h264_packets::split_annexb_nals(data);
        if nals.is_empty() {
            return Ok(());
        }

        // Build AVCC buffer: for each NAL, 4-byte big-endian length + NAL bytes
        let total_len: usize = nals.iter().map(|n| 4 + n.len()).sum();
        let mut avcc_buf = Vec::with_capacity(total_len);
        for nal in &nals {
            let len = nal.len() as u32;
            avcc_buf.extend_from_slice(&len.to_be_bytes());
            avcc_buf.extend_from_slice(nal);
        }

        unsafe {
            // Create CMBlockBuffer
            let mut block_buffer: CMBlockBufferRef = std::ptr::null_mut();
            let status = CMBlockBufferCreateWithMemoryBlock(
                std::ptr::null(),
                avcc_buf.as_ptr() as *mut c_void,
                avcc_buf.len(),
                kCFAllocatorNull as *const c_void,
                std::ptr::null(), // no custom block source
                0,
                avcc_buf.len(),
                0, // flags
                &mut block_buffer,
            );
            if status != 0 || block_buffer.is_null() {
                return Err(format!("CMBlockBufferCreateWithMemoryBlock failed: {}", status));
            }

            // Create CMSampleBuffer
            let pts = CMTime {
                value: pts_ms as i64,
                timescale: 1000,
                flags: kCMTimeFlags_Valid,
                epoch: 0,
            };
            let timing = CMSampleTimingInfo {
                duration: kCMTimeInvalid,
                presentationTimeStamp: pts,
                decodeTimeStamp: kCMTimeInvalid,
            };
            let sample_size = avcc_buf.len();

            let mut sample_buffer: CMSampleBufferRef = std::ptr::null_mut();
            let status = CMSampleBufferCreateReady(
                std::ptr::null(),
                block_buffer,
                self.format_desc,
                1,  // numSamples
                1,  // numSampleTimingEntries
                &timing,
                1,  // numSampleSizeEntries
                &sample_size,
                &mut sample_buffer,
            );

            if status != 0 || sample_buffer.is_null() {
                CFRelease(block_buffer as *const c_void);
                return Err(format!("CMSampleBufferCreateReady failed: {}", status));
            }

            // Decode synchronously (flag 0)
            let mut info_flags: VTDecodeInfoFlags = 0;
            let decode_status = VTDecompressionSessionDecodeFrame(
                self.session,
                sample_buffer,
                0, // synchronous decode
                std::ptr::null_mut(),
                &mut info_flags,
            );

            CFRelease(sample_buffer as *const c_void);
            CFRelease(block_buffer as *const c_void);

            // Keep avcc_buf alive until after decode completes (synchronous)
            drop(avcc_buf);

            if decode_status != 0 {
                return Err(format!("VTDecompressionSessionDecodeFrame failed: {}", decode_status));
            }

            Ok(())
        }
    }

    fn pull_frame(&mut self) -> Result<Option<MseDecodedFrame>, String> {
        let mut q = self.queue.lock().unwrap();
        Ok(q.frames.pop_front())
    }

    fn flush(&mut self) {
        unsafe {
            let _ = VTDecompressionSessionWaitForAsynchronousFrames(self.session);
        }
        // Drain remaining frames
        let mut q = self.queue.lock().unwrap();
        q.frames.clear();
    }
}

impl Drop for AppleH264Decoder {
    fn drop(&mut self) {
        unsafe {
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session as *const c_void);
            CFRelease(self.format_desc as *const c_void);
        }
    }
}
