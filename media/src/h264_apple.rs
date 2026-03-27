use {
    crate::h264_packets,
    makepad_platform::{
        os::apple::apple_sys::*, CameraFrameOwned, CameraFrameRef, EncodedVideoPacketRef,
        MediaVideoEncoder, VideoBitstreamFormat, VideoCodec, VideoEncodeError, VideoEncoderConfig,
        VideoOutputFn, VideoQueuePolicy,
    },
    std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Condvar, Mutex,
        },
    },
};

struct SharedQueue {
    queue: Mutex<VecDeque<EncodeItem>>,
    condvar: Condvar,
}

#[derive(Copy, Clone)]
struct RetainedPixelBuffer(CVPixelBufferRef);

unsafe impl Send for RetainedPixelBuffer {}

enum EncodeItem {
    PixelBuffer {
        pixel_buffer: RetainedPixelBuffer,
        timestamp_ns: u64,
    },
    CpuFrame(CameraFrameOwned),
}

struct AppleH264OutputState {
    output: VideoOutputFn,
    config_id: u32,
    last_emitted_config_id: Option<u32>,
    active_config_annexb: Vec<u8>,
    nal_len_size: usize,
}

pub struct AppleH264Encoder {
    running: Arc<AtomicBool>,
    queue: Arc<SharedQueue>,
    queue_policy: VideoQueuePolicy,
    queue_capacity: usize,
    output_state: Arc<Mutex<AppleH264OutputState>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl AppleH264Encoder {
    pub fn start(config: VideoEncoderConfig, output: VideoOutputFn) -> Option<Self> {
        if config.codec != VideoCodec::H264 {
            return None;
        }
        if config.width == 0 || config.height == 0 || config.fps_num == 0 {
            return None;
        }

        let running = Arc::new(AtomicBool::new(true));
        let queue = Arc::new(SharedQueue {
            queue: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        });

        let output_state = Arc::new(Mutex::new(AppleH264OutputState {
            output,
            config_id: 0,
            last_emitted_config_id: None,
            active_config_annexb: Vec::new(),
            nal_len_size: 4,
        }));

        let running_clone = running.clone();
        let queue_clone = queue.clone();
        let output_state_clone = output_state.clone();

        let worker = std::thread::Builder::new()
            .name("apple-h264-encoder".to_string())
            .spawn(move || {
                worker_loop(config, running_clone, queue_clone, output_state_clone);
            })
            .ok()?;

        Some(Self {
            running,
            queue,
            queue_policy: config.queue_policy,
            queue_capacity: config.queue_capacity.max(1),
            output_state,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn push_pixel_buffer(&self, pixel_buffer: CVPixelBufferRef, timestamp_ns: u64) -> bool {
        if !self.running.load(Ordering::Relaxed) || pixel_buffer.is_null() {
            return false;
        }

        unsafe {
            CVPixelBufferRetain(pixel_buffer);
        }

        let mut q = self.queue.queue.lock().unwrap();
        match self.queue_policy {
            VideoQueuePolicy::LatestWins => {
                if q.len() >= self.queue_capacity {
                    drop_item(q.pop_front());
                }
            }
        }
        q.push_back(EncodeItem::PixelBuffer {
            pixel_buffer: RetainedPixelBuffer(pixel_buffer),
            timestamp_ns,
        });
        self.queue.condvar.notify_one();
        true
    }

    pub fn stop(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            self.queue.condvar.notify_all();
            if let Some(worker) = self.worker.lock().unwrap().take() {
                let _ = worker.join();
            }

            let mut st = self.output_state.lock().unwrap();
            let config_id = st.config_id;
            (st.output)(EncodedVideoPacketRef {
                codec: VideoCodec::H264,
                format: VideoBitstreamFormat::AnnexB,
                pts_ns: 0,
                dts_ns: None,
                is_key: false,
                is_config: false,
                is_eos: true,
                config_id,
                data: &[],
            });
        }
    }
}

impl MediaVideoEncoder for AppleH264Encoder {
    fn push_frame(&self, frame: CameraFrameRef<'_>) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }

        let mut owned = CameraFrameOwned::default();
        if !owned.convert_to_i420(frame) {
            return;
        }

        let mut q = self.queue.queue.lock().unwrap();
        match self.queue_policy {
            VideoQueuePolicy::LatestWins => {
                if q.len() >= self.queue_capacity {
                    drop_item(q.pop_front());
                }
            }
        }
        q.push_back(EncodeItem::CpuFrame(owned));
        self.queue.condvar.notify_one();
    }

    fn push_apple_pixel_buffer(&self, pixel_buffer: CVPixelBufferRef, timestamp_ns: u64) -> bool {
        self.push_pixel_buffer(pixel_buffer, timestamp_ns)
    }

    fn request_keyframe(&self) -> Result<(), VideoEncodeError> {
        Err(VideoEncodeError::UnsupportedCodec)
    }

    fn stop(&self) {
        AppleH264Encoder::stop(self);
    }
}

impl Drop for AppleH264Encoder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn drop_item(item: Option<EncodeItem>) {
    if let Some(EncodeItem::PixelBuffer { pixel_buffer, .. }) = item {
        unsafe {
            CVPixelBufferRelease(pixel_buffer.0);
        }
    }
}

unsafe extern "C" fn compression_output_callback(
    output_refcon: *mut std::ffi::c_void,
    _source_frame_refcon: *mut std::ffi::c_void,
    status: OSStatus,
    _info_flags: VTEncodeInfoFlags,
    sample_buffer: CMSampleBufferRef,
) {
    if status != 0 || sample_buffer.is_null() {
        return;
    }
    if CMSampleBufferDataIsReady(sample_buffer) == NO {
        return;
    }

    let state_arc = &*(output_refcon as *const Arc<Mutex<AppleH264OutputState>>);
    let mut st = state_arc.lock().unwrap();

    let pts = CMSampleBufferGetPresentationTimeStamp(sample_buffer);
    let pts_ns = if pts.timescale > 0 {
        (pts.value.max(0) as u64)
            .saturating_mul(1_000_000_000)
            .saturating_div(pts.timescale as u64)
    } else {
        0
    };

    let format_desc = CMSampleBufferGetFormatDescription(sample_buffer);
    if !format_desc.is_null() {
        let mut set_count: usize = 0;
        let mut nal_len_size: i32 = 0;
        let mut set_ptr: *const u8 = std::ptr::null();
        let mut set_len: usize = 0;
        let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            format_desc,
            0,
            &mut set_ptr,
            &mut set_len,
            &mut set_count,
            &mut nal_len_size,
        );
        if status == 0 && set_count > 0 {
            st.nal_len_size = nal_len_size.max(1) as usize;
            let mut sps = Vec::new();
            let mut pps = Vec::new();
            for i in 0..set_count {
                let mut p_ptr: *const u8 = std::ptr::null();
                let mut p_len: usize = 0;
                let mut dummy_count: usize = 0;
                let mut dummy_len: i32 = 0;
                if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    format_desc,
                    i,
                    &mut p_ptr,
                    &mut p_len,
                    &mut dummy_count,
                    &mut dummy_len,
                ) != 0
                    || p_ptr.is_null()
                    || p_len == 0
                {
                    continue;
                }
                let nal = std::slice::from_raw_parts(p_ptr, p_len);
                match nal[0] & 0x1f {
                    7 => sps.push(nal.to_vec()),
                    8 => pps.push(nal.to_vec()),
                    _ => {}
                }
            }

            if !sps.is_empty() && !pps.is_empty() {
                let cfg = h264_packets::sps_pps_to_annexb(&sps, &pps);
                if st.active_config_annexb != cfg {
                    st.config_id = st.config_id.saturating_add(1);
                    st.active_config_annexb = cfg.clone();
                }
                let config_id = st.config_id;
                if st.last_emitted_config_id != Some(config_id) {
                    (st.output)(EncodedVideoPacketRef {
                        codec: VideoCodec::H264,
                        format: VideoBitstreamFormat::AnnexB,
                        pts_ns,
                        dts_ns: None,
                        is_key: false,
                        is_config: true,
                        is_eos: false,
                        config_id,
                        data: &cfg,
                    });
                    st.last_emitted_config_id = Some(config_id);
                }
            }
        }
    }

    let block_buffer = CMSampleBufferGetDataBuffer(sample_buffer);
    if block_buffer.is_null() {
        return;
    }
    let block_len = CMBlockBufferGetDataLength(block_buffer);
    if block_len <= 0 {
        return;
    }

    let mut raw = vec![0u8; block_len as usize];
    if CMBlockBufferCopyDataBytes(
        block_buffer,
        0,
        block_len,
        raw.as_mut_ptr() as *mut std::ffi::c_void,
    ) != 0
    {
        return;
    }

    let packet_data = if let Some(annexb) = h264_packets::avcc_sample_to_annexb(&raw, st.nal_len_size)
    {
        annexb
    } else {
        raw
    };

    if !h264_packets::starts_with_annexb(&packet_data) {
        return;
    }

    let attachments = CMSampleBufferGetSampleAttachmentsArray(sample_buffer, NO);
    let mut is_key = false;
    if !attachments.is_null() && CFArrayGetCount(attachments) > 0 {
        let dict = CFArrayGetValueAtIndex(attachments, 0) as CFDictionaryRef;
        if !dict.is_null() {
            is_key = CFDictionaryContainsKey(
                dict,
                kCMSampleAttachmentKey_NotSync as *const _ as *const std::ffi::c_void,
            ) == 0;
        }
    }
    if !is_key {
        is_key = h264_packets::contains_idr_annexb(&packet_data);
    }

    let config_id = st.config_id;
    if is_key
        && !st.active_config_annexb.is_empty()
        && st.last_emitted_config_id != Some(config_id)
    {
        let cfg = st.active_config_annexb.clone();
        (st.output)(EncodedVideoPacketRef {
            codec: VideoCodec::H264,
            format: VideoBitstreamFormat::AnnexB,
            pts_ns,
            dts_ns: None,
            is_key: false,
            is_config: true,
            is_eos: false,
            config_id,
            data: &cfg,
        });
        st.last_emitted_config_id = Some(config_id);
    }

    (st.output)(EncodedVideoPacketRef {
        codec: VideoCodec::H264,
        format: VideoBitstreamFormat::AnnexB,
        pts_ns,
        dts_ns: None,
        is_key,
        is_config: false,
        is_eos: false,
        config_id,
        data: &packet_data,
    });
}

fn worker_loop(
    config: VideoEncoderConfig,
    running: Arc<AtomicBool>,
    queue: Arc<SharedQueue>,
    output_state: Arc<Mutex<AppleH264OutputState>>,
) {
    unsafe {
        let mut session: VTCompressionSessionRef = std::ptr::null_mut();
        let callback_ctx = Box::new(output_state.clone());
        let callback_ptr = Box::into_raw(callback_ctx) as *mut std::ffi::c_void;

        let create_status = VTCompressionSessionCreate(
            std::ptr::null(),
            config.width,
            config.height,
            kCMVideoCodecType_H264,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            Some(compression_output_callback),
            callback_ptr,
            &mut session,
        );

        if create_status != 0 || session.is_null() {
            let _ = Box::from_raw(callback_ptr as *mut Arc<Mutex<AppleH264OutputState>>);
            return;
        }

        set_bool_property(session, kVTCompressionPropertyKey_RealTime, true);
        set_bool_property(session, kVTCompressionPropertyKey_AllowFrameReordering, false);
        set_i32_property(
            session,
            kVTCompressionPropertyKey_AverageBitRate,
            config.target_bitrate as i32,
        );
        set_i32_property(
            session,
            kVTCompressionPropertyKey_ExpectedFrameRate,
            config.fps_num as i32,
        );
        set_i32_property(
            session,
            kVTCompressionPropertyKey_MaxKeyFrameInterval,
            config.keyint.max(1),
        );

        let _ = VTCompressionSessionPrepareToEncodeFrames(session);

        loop {
            let item = {
                let mut guard = queue.queue.lock().unwrap();
                while running.load(Ordering::Relaxed) && guard.is_empty() {
                    guard = queue.condvar.wait(guard).unwrap();
                }
                guard.pop_front()
            };

            if let Some(item) = item {
                match item {
                    EncodeItem::PixelBuffer {
                        pixel_buffer,
                        timestamp_ns,
                    } => {
                        let pts = CMTime {
                            value: timestamp_ns as i64,
                            timescale: 1_000_000_000,
                            flags: kCMTimeFlags_Valid,
                            epoch: 0,
                        };
                        let dur = CMTime::default();
                        let _ = VTCompressionSessionEncodeFrame(
                            session,
                            pixel_buffer.0,
                            pts,
                            dur,
                            std::ptr::null(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                        );
                        CVPixelBufferRelease(pixel_buffer.0);
                    }
                    EncodeItem::CpuFrame(frame) => {
                        if frame.width as u32 != config.width || frame.height as u32 != config.height {
                            continue;
                        }
                        if let Some(pixel_buffer) = create_nv12_pixel_buffer_from_i420(&frame) {
                            let pts = CMTime {
                                value: frame.timestamp_ns as i64,
                                timescale: 1_000_000_000,
                                flags: kCMTimeFlags_Valid,
                                epoch: 0,
                            };
                            let dur = CMTime::default();
                            let _ = VTCompressionSessionEncodeFrame(
                                session,
                                pixel_buffer,
                                pts,
                                dur,
                                std::ptr::null(),
                                std::ptr::null_mut(),
                                std::ptr::null_mut(),
                            );
                            CVPixelBufferRelease(pixel_buffer);
                        }
                    }
                }
                continue;
            }

            if !running.load(Ordering::Relaxed) {
                break;
            }
        }

        {
            let mut guard = queue.queue.lock().unwrap();
            while let Some(item) = guard.pop_front() {
                drop_item(Some(item));
            }
        }

        let _ = VTCompressionSessionCompleteFrames(session, CMTime::default());
        VTCompressionSessionInvalidate(session);
        CFRelease(session as *const std::ffi::c_void);

        let _ = Box::from_raw(callback_ptr as *mut Arc<Mutex<AppleH264OutputState>>);
    }
}

unsafe fn set_bool_property(session: VTCompressionSessionRef, key: CFStringRef, value: bool) {
    let v = if value { kCFBooleanTrue } else { kCFBooleanFalse };
    let _ = VTSessionSetProperty(session, key, v as *const std::ffi::c_void);
}

unsafe fn set_i32_property(session: VTCompressionSessionRef, key: CFStringRef, value: i32) {
    let number = CFNumberCreate(
        std::ptr::null(),
        kCFNumberSInt32Type,
        &value as *const _ as *const std::ffi::c_void,
    );
    if !number.is_null() {
        let _ = VTSessionSetProperty(session, key, number as *const std::ffi::c_void);
        CFRelease(number as *const std::ffi::c_void);
    }
}

unsafe fn create_nv12_pixel_buffer_from_i420(frame: &CameraFrameOwned) -> Option<CVPixelBufferRef> {
    if frame.plane_count < 3 || frame.width == 0 || frame.height == 0 {
        return None;
    }

    let mut pixel_buffer: CVPixelBufferRef = std::ptr::null_mut();
    let status = CVPixelBufferCreate(
        std::ptr::null(),
        frame.width,
        frame.height,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        std::ptr::null(),
        &mut pixel_buffer,
    );
    if status != 0 || pixel_buffer.is_null() {
        return None;
    }

    CVPixelBufferLockBaseAddress(pixel_buffer, 0);

    let plane_count = CVPixelBufferGetPlaneCount(pixel_buffer);
    if plane_count < 2 {
        CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
        CVPixelBufferRelease(pixel_buffer);
        return None;
    }

    let dst_y = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 0) as *mut u8;
    let dst_uv = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, 1) as *mut u8;
    let dst_y_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 0);
    let dst_uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, 1);

    if dst_y.is_null() || dst_uv.is_null() {
        CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
        CVPixelBufferRelease(pixel_buffer);
        return None;
    }

    let y_w = frame.width;
    let y_h = frame.height;
    let uv_w = y_w.div_ceil(2);
    let uv_h = y_h.div_ceil(2);

    for row in 0..y_h {
        let src_off = row * frame.planes[0].row_stride;
        let dst_off = row * dst_y_stride;
        std::ptr::copy_nonoverlapping(
            frame.planes[0].bytes[src_off..src_off + y_w].as_ptr(),
            dst_y.add(dst_off),
            y_w,
        );
    }

    for row in 0..uv_h {
        let src_u_off = row * frame.planes[1].row_stride;
        let src_v_off = row * frame.planes[2].row_stride;
        let dst_off = row * dst_uv_stride;
        for col in 0..uv_w {
            *dst_uv.add(dst_off + col * 2) = frame.planes[1].bytes[src_u_off + col];
            *dst_uv.add(dst_off + col * 2 + 1) = frame.planes[2].bytes[src_v_off + col];
        }
    }

    CVPixelBufferUnlockBaseAddress(pixel_buffer, 0);
    Some(pixel_buffer)
}
