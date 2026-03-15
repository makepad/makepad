use {
    self::super::{libc_sys, v4l2_sys::*},
    crate::{
        makepad_live_id::*, thread::SignalToUI, video::*,
        video_encode::camera_video_encoder::VideoEncoder,
    },
    std::ffi::CStr,
    std::os::raw::{c_char, c_int, c_void},
    std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

const NUM_BUFFERS: usize = 4;

struct V4l2CameraDevice {
    path: String,
    desc: VideoInputDesc,
}

struct MmapBuffer {
    ptr: *mut u8,
    length: usize,
}

unsafe impl Send for MmapBuffer {}

// Wrapper for sending raw pointer to the capture thread.
// Safety: the mmap'd memory outlives the thread (we join before munmap).
struct SendPtr(*mut u8);
unsafe impl Send for SendPtr {}

struct V4l2CaptureSession {
    fd: c_int,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    buffers: Vec<MmapBuffer>,
}

impl V4l2CaptureSession {
    fn start(
        input_fn: Arc<Mutex<Option<VideoInputFn>>>,
        frame_input_fn: Arc<Mutex<Option<CameraFrameInputFn>>>,
        video_encoder: Arc<Mutex<Option<VideoEncoder>>>,
        device_path: &str,
        format: VideoFormat,
    ) -> Option<Self> {
        unsafe {
            let path_cstr = std::ffi::CString::new(device_path).ok()?;
            let fd = libc_sys::open(path_cstr.as_ptr() as *const _, libc_sys::O_RDWR);
            if fd < 0 {
                crate::log!(
                    "V4L2: failed to open {}: {}",
                    device_path,
                    std::io::Error::last_os_error()
                );
                return None;
            }

            // Negotiate format: query device capabilities and find a compatible setting
            let format = match Self::negotiate_format(fd, &format) {
                Some(f) => f,
                None => {
                    crate::log!(
                        "V4L2: no compatible format for {} (requested {}x{} {:?})",
                        device_path,
                        format.width,
                        format.height,
                        format.pixel_format
                    );
                    libc_sys::close(fd);
                    return None;
                }
            };

            // Set the negotiated format
            let mut fmt: v4l2_format = std::mem::zeroed();
            fmt.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            fmt.fmt.pix.width = format.width as u32;
            fmt.fmt.pix.height = format.height as u32;
            fmt.fmt.pix.pixelformat = video_pixel_format_to_fourcc(format.pixel_format);
            fmt.fmt.pix.field = V4L2_FIELD_ANY;
            if ioctl(fd, VIDIOC_S_FMT, &mut fmt as *mut _ as *mut c_void) < 0 {
                crate::log!(
                    "V4L2: VIDIOC_S_FMT failed for {} ({}x{} {:?}): {}",
                    device_path,
                    format.width,
                    format.height,
                    format.pixel_format,
                    std::io::Error::last_os_error()
                );
                libc_sys::close(fd);
                return None;
            }

            // Read back what the driver actually accepted
            let format = VideoFormat {
                format_id: format.format_id,
                width: fmt.fmt.pix.width as usize,
                height: fmt.fmt.pix.height as usize,
                pixel_format: fourcc_to_video_pixel_format(fmt.fmt.pix.pixelformat),
                frame_rate: format.frame_rate,
            };

            // Set frame rate if specified
            if let Some(fps) = format.frame_rate {
                let mut parm: v4l2_streamparm = std::mem::zeroed();
                parm.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                if ioctl(fd, VIDIOC_S_PARM, &mut parm as *mut _ as *mut c_void) >= 0
                    && (parm.parm.capture.capability & V4L2_CAP_TIMEPERFRAME) != 0
                {
                    parm.parm.capture.timeperframe.numerator = 1;
                    parm.parm.capture.timeperframe.denominator = fps as u32;
                    ioctl(fd, VIDIOC_S_PARM, &mut parm as *mut _ as *mut c_void);
                }
            }

            // Request mmap buffers
            let mut req: v4l2_requestbuffers = std::mem::zeroed();
            req.count = NUM_BUFFERS as u32;
            req.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            req.memory = V4L2_MEMORY_MMAP;
            if ioctl(fd, VIDIOC_REQBUFS, &mut req as *mut _ as *mut c_void) < 0 {
                crate::log!(
                    "V4L2: VIDIOC_REQBUFS failed for {}: {}",
                    device_path,
                    std::io::Error::last_os_error()
                );
                libc_sys::close(fd);
                return None;
            }

            let buf_count = req.count as usize;
            let mut buffers: Vec<MmapBuffer> = Vec::with_capacity(buf_count);

            // Query and mmap each buffer
            for i in 0..buf_count {
                let mut buf: v4l2_buffer = std::mem::zeroed();
                buf.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                buf.memory = V4L2_MEMORY_MMAP;
                buf.index = i as u32;
                if ioctl(fd, VIDIOC_QUERYBUF, &mut buf as *mut _ as *mut c_void) < 0 {
                    crate::log!(
                        "V4L2: VIDIOC_QUERYBUF failed for buffer {}: {}",
                        i,
                        std::io::Error::last_os_error()
                    );
                    for b in &buffers {
                        libc_sys::munmap(b.ptr as *mut c_void, b.length);
                    }
                    libc_sys::close(fd);
                    return None;
                }

                let ptr = libc_sys::mmap(
                    std::ptr::null_mut(),
                    buf.length as usize,
                    libc_sys::PROT_READ | libc_sys::PROT_WRITE,
                    libc_sys::MAP_SHARED,
                    fd,
                    buf.m.offset as libc_sys::off_t,
                );
                if ptr == libc_sys::MAP_FAILED {
                    crate::log!(
                        "V4L2: mmap failed for buffer {}: {}",
                        i,
                        std::io::Error::last_os_error()
                    );
                    for b in &buffers {
                        libc_sys::munmap(b.ptr as *mut c_void, b.length);
                    }
                    libc_sys::close(fd);
                    return None;
                }

                buffers.push(MmapBuffer {
                    ptr: ptr as *mut u8,
                    length: buf.length as usize,
                });
            }

            // Queue all buffers
            for i in 0..buf_count {
                let mut buf: v4l2_buffer = std::mem::zeroed();
                buf.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                buf.memory = V4L2_MEMORY_MMAP;
                buf.index = i as u32;
                if ioctl(fd, VIDIOC_QBUF, &mut buf as *mut _ as *mut c_void) < 0 {
                    crate::log!("V4L2: VIDIOC_QBUF failed for buffer {}", i);
                }
            }

            // Stream on
            let mut buf_type: c_int = V4L2_BUF_TYPE_VIDEO_CAPTURE as c_int;
            if ioctl(fd, VIDIOC_STREAMON, &mut buf_type as *mut _ as *mut c_void) < 0 {
                crate::log!(
                    "V4L2: VIDIOC_STREAMON failed for {}: {}",
                    device_path,
                    std::io::Error::last_os_error()
                );
                for b in &buffers {
                    libc_sys::munmap(b.ptr as *mut c_void, b.length);
                }
                libc_sys::close(fd);
                return None;
            }

            // Capture thread receives buffer pointers and fd
            let running = Arc::new(AtomicBool::new(true));
            let running_clone = running.clone();

            // Collect buffer info for the thread (raw pointers wrapped for Send)
            let thread_bufs: Vec<SendPtr> = buffers.iter().map(|b| SendPtr(b.ptr)).collect();

            let thread = std::thread::spawn(move || {
                Self::capture_loop(
                    fd,
                    format,
                    input_fn,
                    frame_input_fn,
                    video_encoder,
                    &thread_bufs,
                    &running_clone,
                );
            });

            Some(Self {
                fd,
                running,
                thread: Some(thread),
                buffers,
            })
        }
    }

    /// Query device for supported formats/resolutions and pick the best match.
    /// Tries the requested format first, then falls back to device-supported alternatives.
    unsafe fn negotiate_format(fd: c_int, requested: &VideoFormat) -> Option<VideoFormat> {
        // Enumerate all supported (pixelformat, width, height) tuples
        let mut supported: Vec<(u32, u32, u32)> = Vec::new();
        let mut fmt_index = 0u32;
        loop {
            let mut fmtdesc: v4l2_fmtdesc = std::mem::zeroed();
            fmtdesc.index = fmt_index;
            fmtdesc.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if ioctl(fd, VIDIOC_ENUM_FMT, &mut fmtdesc as *mut _ as *mut c_void) < 0 {
                break;
            }
            let pixfmt = fmtdesc.pixelformat;

            let mut size_index = 0u32;
            loop {
                let mut frmsize: v4l2_frmsizeenum = std::mem::zeroed();
                frmsize.index = size_index;
                frmsize.pixel_format = pixfmt;
                if ioctl(
                    fd,
                    VIDIOC_ENUM_FRAMESIZES,
                    &mut frmsize as *mut _ as *mut c_void,
                ) < 0
                {
                    break;
                }
                if frmsize.type_ == V4L2_FRMSIZE_TYPE_DISCRETE {
                    let w = frmsize.u.discrete.width;
                    let h = frmsize.u.discrete.height;
                    supported.push((pixfmt, w, h));
                }
                size_index += 1;
            }

            // Device reports format but no discrete sizes: accept any resolution
            if size_index == 0 {
                supported.push((pixfmt, 0, 0));
            }

            fmt_index += 1;
        }

        // If enumeration returned nothing, let the driver decide via G_FMT
        if supported.is_empty() {
            crate::log!("V4L2: no formats enumerated, trying G_FMT fallback");
            let mut fmt: v4l2_format = std::mem::zeroed();
            fmt.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if ioctl(fd, VIDIOC_G_FMT, &mut fmt as *mut _ as *mut c_void) >= 0 {
                let pf = fourcc_to_video_pixel_format(fmt.fmt.pix.pixelformat);
                if !matches!(pf, VideoPixelFormat::Unsupported(_)) {
                    return Some(VideoFormat {
                        format_id: requested.format_id,
                        width: fmt.fmt.pix.width as usize,
                        height: fmt.fmt.pix.height as usize,
                        pixel_format: pf,
                        frame_rate: requested.frame_rate,
                    });
                }
            }
            return None;
        }

        let req_fourcc = video_pixel_format_to_fourcc(requested.pixel_format);
        let req_w = requested.width as u32;
        let req_h = requested.height as u32;

        // 1. Exact match (format + resolution)
        if supported
            .iter()
            .any(|&(f, w, h)| f == req_fourcc && (w == req_w && h == req_h || w == 0))
        {
            return Some(*requested);
        }

        // 2. Same pixel format, different resolution — pick closest
        if let Some(fmt) = Self::pick_closest_resolution(&supported, req_fourcc, req_w, req_h) {
            return Some(VideoFormat {
                format_id: requested.format_id,
                width: fmt.1 as usize,
                height: fmt.2 as usize,
                pixel_format: requested.pixel_format,
                frame_rate: requested.frame_rate,
            });
        }

        // 3. Different pixel format — prefer YUYV, MJPEG, NV12, YUV420 in order
        let preferred = [
            V4L2_PIX_FMT_YUYV,
            V4L2_PIX_FMT_MJPEG,
            V4L2_PIX_FMT_NV12,
            V4L2_PIX_FMT_YUV420,
        ];
        for &pf in &preferred {
            if let Some(fmt) = Self::pick_closest_resolution(&supported, pf, req_w, req_h) {
                return Some(VideoFormat {
                    format_id: requested.format_id,
                    width: fmt.1 as usize,
                    height: fmt.2 as usize,
                    pixel_format: fourcc_to_video_pixel_format(pf),
                    frame_rate: requested.frame_rate,
                });
            }
        }

        // 4. Take whatever the device offers first
        let (pf, w, h) = supported[0];
        let pixel_format = fourcc_to_video_pixel_format(pf);
        let (w, h) = if w == 0 { (req_w, req_h) } else { (w, h) };
        Some(VideoFormat {
            format_id: requested.format_id,
            width: w as usize,
            height: h as usize,
            pixel_format,
            frame_rate: requested.frame_rate,
        })
    }

    /// From entries matching `pixfmt`, return the one closest to (target_w, target_h).
    fn pick_closest_resolution(
        supported: &[(u32, u32, u32)],
        pixfmt: u32,
        target_w: u32,
        target_h: u32,
    ) -> Option<(u32, u32, u32)> {
        supported
            .iter()
            .filter(|&&(f, _, _)| f == pixfmt)
            .min_by_key(|&&(_, w, h)| {
                if w == 0 {
                    return 0i64;
                } // wildcard size — perfect match
                let dw = w as i64 - target_w as i64;
                let dh = h as i64 - target_h as i64;
                dw * dw + dh * dh
            })
            .map(|&entry| {
                if entry.1 == 0 {
                    (entry.0, target_w, target_h)
                } else {
                    entry
                }
            })
    }

    fn capture_loop(
        fd: c_int,
        format: VideoFormat,
        input_fn: Arc<Mutex<Option<VideoInputFn>>>,
        frame_input_fn: Arc<Mutex<Option<CameraFrameInputFn>>>,
        video_encoder: Arc<Mutex<Option<VideoEncoder>>>,
        buffers: &[SendPtr],
        running: &AtomicBool,
    ) {
        while running.load(Ordering::Relaxed) {
            unsafe {
                let mut pfd = pollfd {
                    fd,
                    events: POLLIN,
                    revents: 0,
                };
                let ret = poll(&mut pfd, 1, 200);
                if ret <= 0 {
                    continue;
                }

                let mut buf: v4l2_buffer = std::mem::zeroed();
                buf.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                buf.memory = V4L2_MEMORY_MMAP;
                if ioctl(fd, VIDIOC_DQBUF, &mut buf as *mut _ as *mut c_void) < 0 {
                    continue;
                }

                let idx = buf.index as usize;
                if idx < buffers.len() {
                    let ptr = buffers[idx].0;
                    let used = buf.bytesused as usize;
                    let timestamp_ns = (buf.timestamp.tv_sec.max(0) as u64)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(
                            (buf.timestamp.tv_usec.max(0) as u64).saturating_mul(1_000),
                        );

                    let raw = std::slice::from_raw_parts(ptr, used);
                    let frame_ref = match format.pixel_format {
                        VideoPixelFormat::YUV420 => {
                            let y_size = format.width * format.height;
                            let cw = format.width.div_ceil(2);
                            let ch = format.height.div_ceil(2);
                            let uv_size = cw * ch;
                            if raw.len() >= y_size + uv_size * 2 {
                                Some(CameraFrameRef {
                                    timestamp_ns,
                                    width: format.width,
                                    height: format.height,
                                    layout: CameraFrameLayout::I420,
                                    matrix: CameraColorMatrix::BT601,
                                    plane_count: 3,
                                    planes: [
                                        CameraFramePlaneRef {
                                            bytes: &raw[..y_size],
                                            row_stride: format.width,
                                            pixel_stride: 1,
                                        },
                                        CameraFramePlaneRef {
                                            bytes: &raw[y_size..y_size + uv_size],
                                            row_stride: cw,
                                            pixel_stride: 1,
                                        },
                                        CameraFramePlaneRef {
                                            bytes: &raw[y_size + uv_size..y_size + uv_size * 2],
                                            row_stride: cw,
                                            pixel_stride: 1,
                                        },
                                    ],
                                })
                            } else {
                                None
                            }
                        }
                        VideoPixelFormat::NV12 => {
                            let y_size = format.width * format.height;
                            let cw = format.width.div_ceil(2);
                            let ch = format.height.div_ceil(2);
                            let uv_size = cw * ch * 2;
                            if raw.len() >= y_size + uv_size {
                                Some(CameraFrameRef {
                                    timestamp_ns,
                                    width: format.width,
                                    height: format.height,
                                    layout: CameraFrameLayout::NV12,
                                    matrix: CameraColorMatrix::BT601,
                                    plane_count: 2,
                                    planes: [
                                        CameraFramePlaneRef {
                                            bytes: &raw[..y_size],
                                            row_stride: format.width,
                                            pixel_stride: 1,
                                        },
                                        CameraFramePlaneRef {
                                            bytes: &raw[y_size..y_size + uv_size],
                                            row_stride: format.width,
                                            pixel_stride: 2,
                                        },
                                        CameraFramePlaneRef::empty(),
                                    ],
                                })
                            } else {
                                None
                            }
                        }
                        VideoPixelFormat::YUY2 => Some(CameraFrameRef {
                            timestamp_ns,
                            width: format.width,
                            height: format.height,
                            layout: CameraFrameLayout::YUY2,
                            matrix: CameraColorMatrix::BT601,
                            plane_count: 1,
                            planes: [
                                CameraFramePlaneRef {
                                    bytes: raw,
                                    row_stride: format.width * 2,
                                    pixel_stride: 2,
                                },
                                CameraFramePlaneRef::empty(),
                                CameraFramePlaneRef::empty(),
                            ],
                        }),
                        VideoPixelFormat::MJPEG => Some(CameraFrameRef {
                            timestamp_ns,
                            width: format.width,
                            height: format.height,
                            layout: CameraFrameLayout::Mjpeg,
                            matrix: CameraColorMatrix::Unknown,
                            plane_count: 1,
                            planes: [
                                CameraFramePlaneRef {
                                    bytes: raw,
                                    row_stride: used,
                                    pixel_stride: 1,
                                },
                                CameraFramePlaneRef::empty(),
                                CameraFramePlaneRef::empty(),
                            ],
                        }),
                        _ => None,
                    };

                    if let Some(frame_ref) = frame_ref {
                        if let Some(cb) = &mut *frame_input_fn.lock().unwrap() {
                            cb(frame_ref);
                        }
                        if let Some(enc) = &*video_encoder.lock().unwrap() {
                            enc.push_frame(frame_ref);
                        }
                    }

                    if let Some(cb) = &mut *input_fn.lock().unwrap() {
                        match format.pixel_format {
                            VideoPixelFormat::MJPEG => {
                                let data = std::slice::from_raw_parts(ptr, used);
                                cb(VideoBufferRef {
                                    format,
                                    data: VideoBufferRefData::U8(data),
                                });
                            }
                            _ => {
                                let data = std::slice::from_raw_parts(ptr as *const u32, used / 4);
                                cb(VideoBufferRef {
                                    format,
                                    data: VideoBufferRefData::U32(data),
                                });
                            }
                        }
                    }
                }

                // Re-queue
                if ioctl(fd, VIDIOC_QBUF, &mut buf as *mut _ as *mut c_void) < 0 {
                    break;
                }
            }
        }
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            let mut buf_type: c_int = V4L2_BUF_TYPE_VIDEO_CAPTURE as c_int;
            ioctl(
                self.fd,
                VIDIOC_STREAMOFF,
                &mut buf_type as *mut _ as *mut c_void,
            );
            for b in &self.buffers {
                libc_sys::munmap(b.ptr as *mut c_void, b.length);
            }
            libc_sys::close(self.fd);
        }
    }
}

pub struct V4l2CameraAccess {
    pub video_input_cb: [Arc<Mutex<Option<VideoInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub camera_frame_input_cb: [Arc<Mutex<Option<CameraFrameInputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub video_output_cb: [Arc<Mutex<Option<VideoOutputFn>>>; MAX_VIDEO_DEVICE_INDEX],
    pub video_encoder_config: [Arc<Mutex<Option<VideoEncoderConfig>>>; MAX_VIDEO_DEVICE_INDEX],
    video_encoder: [Arc<Mutex<Option<VideoEncoder>>>; MAX_VIDEO_DEVICE_INDEX],
    devices: Vec<V4l2CameraDevice>,
    sessions: Vec<V4l2CaptureSession>,
}

impl V4l2CameraAccess {
    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        let signal = change_signal.clone();
        std::thread::spawn(move || {
            Self::watch_devices(signal);
        });

        change_signal.set();

        Arc::new(Mutex::new(Self {
            video_input_cb: Default::default(),
            camera_frame_input_cb: Default::default(),
            video_output_cb: Default::default(),
            video_encoder_config: Default::default(),
            video_encoder: Default::default(),
            devices: Default::default(),
            sessions: Default::default(),
        }))
    }

    pub fn use_video_input(&mut self, inputs: &[(VideoInputId, VideoFormatId)]) {
        for session in &mut self.sessions {
            session.stop();
        }
        self.sessions.clear();

        for slot in &self.video_encoder {
            *slot.lock().unwrap() = None;
        }

        for (index, (input_id, format_id)) in inputs.iter().enumerate() {
            if let Some(device) = self.devices.iter().find(|d| d.desc.input_id == *input_id) {
                if let Some(format) = device
                    .desc
                    .formats
                    .iter()
                    .find(|f| f.format_id == *format_id)
                {
                    if let (Some(mut config), true) = (
                        *self.video_encoder_config[index].lock().unwrap(),
                        self.video_output_cb[index].lock().unwrap().is_some(),
                    ) {
                        config.width = format.width as u32;
                        config.height = format.height as u32;
                        if let Some(fps) = format.frame_rate {
                            config.fps_num = fps.max(1.0).round() as u32;
                            config.fps_den = 1;
                        }
                        config.source = VideoEncodeSource::Camera {
                            input_id: *input_id,
                            format_id: *format_id,
                        };
                        let output_cb = self.video_output_cb[index].clone();
                        let encoder = VideoEncoder::start(
                            config,
                            Box::new(move |packet| {
                                if let Some(cb) = &mut *output_cb.lock().unwrap() {
                                    cb(packet);
                                }
                            }),
                        );
                        if encoder.is_none() {
                            crate::error!(
                                "linux camera video encoder unavailable for slot {}",
                                index
                            );
                        }
                        *self.video_encoder[index].lock().unwrap() = encoder;
                    }

                    if let Some(session) = V4l2CaptureSession::start(
                        self.video_input_cb[index].clone(),
                        self.camera_frame_input_cb[index].clone(),
                        self.video_encoder[index].clone(),
                        &device.path,
                        *format,
                    ) {
                        self.sessions.push(session);
                    }
                }
            }
        }
    }

    pub fn get_updated_descs(&mut self) -> Vec<VideoInputDesc> {
        self.devices.clear();
        for i in 0..64 {
            let path = format!("/dev/video{}", i);
            if let Some(device) = Self::probe_device(&path) {
                self.devices.push(device);
            }
        }
        self.devices.iter().map(|d| d.desc.clone()).collect()
    }

    fn probe_device(path: &str) -> Option<V4l2CameraDevice> {
        unsafe {
            let path_cstr = std::ffi::CString::new(path).ok()?;
            let fd = libc_sys::open(path_cstr.as_ptr() as *const _, libc_sys::O_RDWR);
            if fd < 0 {
                return None;
            }

            let result = Self::probe_device_fd(fd, path);
            libc_sys::close(fd);
            result
        }
    }

    unsafe fn probe_device_fd(fd: c_int, path: &str) -> Option<V4l2CameraDevice> {
        // Query capabilities
        let mut cap: v4l2_capability = std::mem::zeroed();
        if ioctl(fd, VIDIOC_QUERYCAP, &mut cap as *mut _ as *mut c_void) < 0 {
            return None;
        }

        let caps = if (cap.capabilities & V4L2_CAP_DEVICE_CAPS) != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };

        if (caps & V4L2_CAP_VIDEO_CAPTURE) == 0 {
            return None;
        }

        let name = cstr_from_bytes(&cap.card);

        // Enumerate formats
        let mut formats = Vec::new();
        let mut fmt_index = 0u32;
        loop {
            let mut fmtdesc: v4l2_fmtdesc = std::mem::zeroed();
            fmtdesc.index = fmt_index;
            fmtdesc.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if ioctl(fd, VIDIOC_ENUM_FMT, &mut fmtdesc as *mut _ as *mut c_void) < 0 {
                break;
            }

            let pixel_format = fourcc_to_video_pixel_format(fmtdesc.pixelformat);

            // Enumerate frame sizes for this format
            let mut size_index = 0u32;
            loop {
                let mut frmsize: v4l2_frmsizeenum = std::mem::zeroed();
                frmsize.index = size_index;
                frmsize.pixel_format = fmtdesc.pixelformat;
                if ioctl(
                    fd,
                    VIDIOC_ENUM_FRAMESIZES,
                    &mut frmsize as *mut _ as *mut c_void,
                ) < 0
                {
                    break;
                }

                if frmsize.type_ == V4L2_FRMSIZE_TYPE_DISCRETE {
                    let width = frmsize.u.discrete.width;
                    let height = frmsize.u.discrete.height;

                    // Enumerate frame intervals for this (format, size)
                    let mut ival_index = 0u32;
                    let mut found_interval = false;
                    loop {
                        let mut frmival: v4l2_frmivalenum = std::mem::zeroed();
                        frmival.index = ival_index;
                        frmival.pixel_format = fmtdesc.pixelformat;
                        frmival.width = width;
                        frmival.height = height;
                        if ioctl(
                            fd,
                            VIDIOC_ENUM_FRAMEINTERVALS,
                            &mut frmival as *mut _ as *mut c_void,
                        ) < 0
                        {
                            break;
                        }

                        if frmival.type_ == V4L2_FRMIVAL_TYPE_DISCRETE {
                            let fract = frmival.u.discrete;
                            if fract.numerator > 0 {
                                let frame_rate = fract.denominator as f64 / fract.numerator as f64;
                                let format_id = LiveId::from_str(&format!(
                                    "{} {} {:?} {}",
                                    width, height, pixel_format, frame_rate
                                ))
                                .into();
                                formats.push(VideoFormat {
                                    format_id,
                                    width: width as usize,
                                    height: height as usize,
                                    pixel_format,
                                    frame_rate: Some(frame_rate),
                                });
                                found_interval = true;
                            }
                        }

                        ival_index += 1;
                    }

                    // If no intervals enumerated, add format without frame rate
                    if !found_interval {
                        let format_id =
                            LiveId::from_str(&format!("{} {} {:?}", width, height, pixel_format))
                                .into();
                        formats.push(VideoFormat {
                            format_id,
                            width: width as usize,
                            height: height as usize,
                            pixel_format,
                            frame_rate: None,
                        });
                    }
                }

                size_index += 1;
            }

            fmt_index += 1;
        }

        if formats.is_empty() {
            return None;
        }

        let input_id = LiveId::from_str(path).into();
        Some(V4l2CameraDevice {
            path: path.to_string(),
            desc: VideoInputDesc {
                input_id,
                name,
                formats,
            },
        })
    }

    fn watch_devices(change_signal: SignalToUI) {
        unsafe {
            let fd = inotify_init1(IN_NONBLOCK);
            if fd < 0 {
                // Fallback: poll periodically
                Self::poll_devices(change_signal);
                return;
            }

            let dev_path = b"/dev\0";
            let wd = inotify_add_watch(
                fd,
                dev_path.as_ptr() as *const c_char,
                IN_CREATE | IN_DELETE,
            );
            if wd < 0 {
                libc_sys::close(fd);
                Self::poll_devices(change_signal);
                return;
            }

            let mut buf = [0u8; 4096];
            loop {
                let mut pfd = pollfd {
                    fd,
                    events: POLLIN,
                    revents: 0,
                };
                let ret = poll(&mut pfd, 1, 2000);
                if ret <= 0 {
                    continue;
                }

                let n = libc_sys::read(fd, buf.as_mut_ptr() as *mut c_void, buf.len());
                if n <= 0 {
                    continue;
                }

                let mut offset = 0usize;
                let mut found_video = false;
                while offset + std::mem::size_of::<inotify_event>() <= n as usize {
                    let event = &*(buf.as_ptr().add(offset) as *const inotify_event);
                    let name_offset = offset + std::mem::size_of::<inotify_event>();
                    let event_len = std::mem::size_of::<inotify_event>() + event.len as usize;

                    if event.len > 0 && name_offset < n as usize {
                        let name_ptr = buf.as_ptr().add(name_offset) as *const c_char;
                        if let Ok(name_str) = CStr::from_ptr(name_ptr).to_str() {
                            if name_str.starts_with("video") {
                                found_video = true;
                            }
                        }
                    }

                    offset += event_len;
                }

                if found_video {
                    // Wait briefly for the device node to be ready
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    change_signal.set();
                }
            }
        }
    }

    fn poll_devices(change_signal: SignalToUI) {
        let mut last_count = 0usize;
        loop {
            let mut count = 0usize;
            for i in 0..64 {
                let path = format!("/dev/video{}\0", i);
                let fd = unsafe { libc_sys::open(path.as_ptr() as *const _, libc_sys::O_RDWR) };
                if fd >= 0 {
                    count += 1;
                    unsafe { libc_sys::close(fd) };
                }
            }
            if count != last_count {
                last_count = count;
                change_signal.set();
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }
}

fn fourcc_to_video_pixel_format(fourcc: u32) -> VideoPixelFormat {
    match fourcc {
        V4L2_PIX_FMT_YUYV => VideoPixelFormat::YUY2,
        V4L2_PIX_FMT_MJPEG => VideoPixelFormat::MJPEG,
        V4L2_PIX_FMT_NV12 => VideoPixelFormat::NV12,
        V4L2_PIX_FMT_YUV420 => VideoPixelFormat::YUV420,
        V4L2_PIX_FMT_RGB24 => VideoPixelFormat::RGB24,
        V4L2_PIX_FMT_GREY => VideoPixelFormat::GRAY,
        other => VideoPixelFormat::Unsupported(other),
    }
}

fn video_pixel_format_to_fourcc(format: VideoPixelFormat) -> u32 {
    match format {
        VideoPixelFormat::YUY2 => V4L2_PIX_FMT_YUYV,
        VideoPixelFormat::MJPEG => V4L2_PIX_FMT_MJPEG,
        VideoPixelFormat::NV12 => V4L2_PIX_FMT_NV12,
        VideoPixelFormat::YUV420 => V4L2_PIX_FMT_YUV420,
        VideoPixelFormat::RGB24 => V4L2_PIX_FMT_RGB24,
        VideoPixelFormat::GRAY => V4L2_PIX_FMT_GREY,
        VideoPixelFormat::Unsupported(fcc) => fcc,
    }
}

unsafe fn cstr_from_bytes(bytes: &[u8]) -> String {
    // UTF-8 Lossy: V4L2 device names are kernel byte arrays for display only
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
