//! V4L2 mmap capture. Device setup, frame copies and teardown run on the media worker.
use super::{libc_sys, v4l2_sys::*};
use crate::fake_camera::{self, FakeCapture};
use crate::media::{
    camera_layout, name_hash, pack_camera_frame, CameraFrameData, CameraPlane, VideoFormatInfo,
    VideoInputInfo, VideoPixel,
};
use std::{
    collections::HashMap,
    os::raw::{c_int, c_void},
};
const NUM_BUFFERS: usize = 3;
struct Device {
    path: String,
    desc: VideoInputInfo,
}
struct MmapBuffer {
    ptr: *mut u8,
    length: usize,
}
struct Capture {
    fd: c_int,
    path: String,
    format: VideoFormatInfo,
    buffers: Vec<MmapBuffer>,
    streaming: bool,
    stride: u32,
    frames: u64,
    started: Option<std::time::Instant>,
    warned: bool,
}
impl Capture {
    fn open(device_path: &str, format: VideoFormatInfo) -> Result<Self, String> {
        unsafe {
            let path_cstr = std::ffi::CString::new(device_path)
                .map_err(|_| format!("bad device path {device_path}"))?;
            let fd = libc_sys::open(
                path_cstr.as_ptr() as *const _,
                libc_sys::O_RDWR | libc_sys::O_NONBLOCK,
            );
            if fd < 0 {
                return Err(format!(
                    "failed to open {device_path}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // Every failure below closes the descriptor.
            let fail = |fd: c_int, buffers: &[MmapBuffer], why: String| -> Result<Self, String> {
                for b in buffers {
                    libc_sys::munmap(b.ptr as *mut c_void, b.length);
                }
                libc_sys::close(fd);
                Err(why)
            };

            // Negotiate format: query device capabilities and find a compatible setting
            let Some(format) = Self::negotiate_format(fd, &format) else {
                return fail(
                    fd,
                    &[],
                    format!(
                        "no compatible format for {device_path} (requested {}x{} {:?})",
                        format.width, format.height, format.pixel
                    ),
                );
            };

            // Set the negotiated format
            let mut fmt: v4l2_format = std::mem::zeroed();
            fmt.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            fmt.fmt.pix.width = format.width as u32;
            fmt.fmt.pix.height = format.height as u32;
            fmt.fmt.pix.pixelformat = video_pixel_format_to_fourcc(format.pixel);
            fmt.fmt.pix.field = V4L2_FIELD_ANY;
            if ioctl(fd, VIDIOC_S_FMT, &mut fmt as *mut _ as *mut c_void) < 0 {
                return fail(
                    fd,
                    &[],
                    format!(
                        "VIDIOC_S_FMT failed for {device_path} ({}x{} {:?}): {}",
                        format.width,
                        format.height,
                        format.pixel,
                        std::io::Error::last_os_error()
                    ),
                );
            }

            // Read back what the driver actually accepted
            let mut format = VideoFormatInfo {
                id: format.id,
                width: fmt.fmt.pix.width as u32,
                height: fmt.fmt.pix.height as u32,
                pixel: fourcc_to_video_pixel_format(fmt.fmt.pix.pixelformat),
                frame_rate: format.frame_rate,
            };

            // The frame rate: ask the driver for it, and keep the rate it
            // says it will run at.
            if let Some(fps) = format.frame_rate {
                let mut parm: v4l2_streamparm = std::mem::zeroed();
                parm.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                if ioctl(fd, VIDIOC_G_PARM, &mut parm as *mut _ as *mut c_void) >= 0
                    && (parm.parm.capture.capability & V4L2_CAP_TIMEPERFRAME) != 0
                {
                    parm.parm.capture.timeperframe.numerator = 1;
                    parm.parm.capture.timeperframe.denominator = fps.round().max(1.0) as u32;
                    if ioctl(fd, VIDIOC_S_PARM, &mut parm as *mut _ as *mut c_void) >= 0 {
                        let got = parm.parm.capture.timeperframe;
                        if got.numerator > 0 {
                            format.frame_rate = Some(got.denominator as f64 / got.numerator as f64);
                        }
                    }
                }
            }

            // Request mmap buffers
            let mut req: v4l2_requestbuffers = std::mem::zeroed();
            req.count = NUM_BUFFERS as u32;
            req.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            req.memory = V4L2_MEMORY_MMAP;
            if ioctl(fd, VIDIOC_REQBUFS, &mut req as *mut _ as *mut c_void) < 0 {
                return fail(
                    fd,
                    &[],
                    format!(
                        "VIDIOC_REQBUFS failed for {device_path}: {}",
                        std::io::Error::last_os_error()
                    ),
                );
            }

            let buf_count = req.count as usize;
            if buf_count == 0 {
                return fail(fd, &[], "driver supplied no capture buffers".into());
            }
            let mut buffers: Vec<MmapBuffer> = Vec::with_capacity(buf_count);

            // Query and mmap each buffer
            for i in 0..buf_count {
                let mut buf: v4l2_buffer = std::mem::zeroed();
                buf.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                buf.memory = V4L2_MEMORY_MMAP;
                buf.index = i as u32;
                if ioctl(fd, VIDIOC_QUERYBUF, &mut buf as *mut _ as *mut c_void) < 0 {
                    return fail(
                        fd,
                        &buffers,
                        format!(
                            "VIDIOC_QUERYBUF failed for buffer {i}: {}",
                            std::io::Error::last_os_error()
                        ),
                    );
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
                    return fail(
                        fd,
                        &buffers,
                        format!(
                            "mmap failed for buffer {i}: {}",
                            std::io::Error::last_os_error()
                        ),
                    );
                }
                buffers.push(MmapBuffer {
                    ptr: ptr as *mut u8,
                    length: buf.length as usize,
                });
            }

            Ok(Self {
                fd,
                path: device_path.to_string(),
                format,
                buffers,
                streaming: false,
                stride: fmt.fmt.pix.bytesperline,
                frames: 0,
                started: None,
                warned: false,
            })
        }
    }

    /// Query device for supported formats/resolutions and pick the best match.
    /// Tries the requested format first, then falls back to device-supported alternatives.
    unsafe fn negotiate_format(fd: c_int, requested: &VideoFormatInfo) -> Option<VideoFormatInfo> {
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
            eprintln!("V4L2: no formats enumerated, trying G_FMT fallback");
            let mut fmt: v4l2_format = std::mem::zeroed();
            fmt.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if ioctl(fd, VIDIOC_G_FMT, &mut fmt as *mut _ as *mut c_void) >= 0 {
                let pf = fourcc_to_video_pixel_format(fmt.fmt.pix.pixelformat);
                if !matches!(pf, VideoPixel::Unsupported(_)) {
                    return Some(VideoFormatInfo {
                        id: requested.id,
                        width: fmt.fmt.pix.width as u32,
                        height: fmt.fmt.pix.height as u32,
                        pixel: pf,
                        frame_rate: requested.frame_rate,
                    });
                }
            }
            return None;
        }

        let req_fourcc = video_pixel_format_to_fourcc(requested.pixel);
        let req_w = requested.width as u32;
        let req_h = requested.height as u32;

        // 1. Exact match (format + resolution)
        if supported
            .iter()
            .any(|&(f, w, h)| f == req_fourcc && (w == req_w && h == req_h || w == 0))
        {
            return Some(requested.clone());
        }

        // 2. Same pixel format, different resolution — pick closest
        if let Some(fmt) = Self::pick_closest_resolution(&supported, req_fourcc, req_w, req_h) {
            return Some(VideoFormatInfo {
                id: requested.id,
                width: fmt.1,
                height: fmt.2,
                pixel: requested.pixel,
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
                return Some(VideoFormatInfo {
                    id: requested.id,
                    width: fmt.1,
                    height: fmt.2,
                    pixel: fourcc_to_video_pixel_format(pf),
                    frame_rate: requested.frame_rate,
                });
            }
        }

        // 4. Take whatever the device offers first
        let (pf, w, h) = supported[0];
        let pixel_format = fourcc_to_video_pixel_format(pf);
        let (w, h) = if w == 0 { (req_w, req_h) } else { (w, h) };
        Some(VideoFormatInfo {
            id: requested.id,
            width: w,
            height: h,
            pixel: pixel_format,
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

    fn start(&mut self) -> Result<(), String> {
        unsafe {
            for i in 0..self.buffers.len() {
                let mut b: v4l2_buffer = std::mem::zeroed();
                b.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                b.memory = V4L2_MEMORY_MMAP;
                b.index = i as u32;
                if ioctl(self.fd, VIDIOC_QBUF, (&mut b as *mut v4l2_buffer).cast::<std::ffi::c_void>()) < 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
            }
            let mut kind = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if ioctl(self.fd, VIDIOC_STREAMON, (&mut kind as *mut u32).cast::<std::ffi::c_void>()) < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            self.streaming = true;
            self.started = Some(std::time::Instant::now());
            Ok(())
        }
    }
    fn poll(&mut self) -> Option<Vec<u8>> {
        let frame = self.poll_frame();
        match &frame {
            Some(bytes) => {
                self.frames += 1;
                if self.frames == 1 {
                    eprintln!(
                        "[hal-camera] {}: first frame, {} bytes, {:?} after streaming started",
                        self.path,
                        bytes.len(),
                        self.started.map(|t| t.elapsed())
                    );
                }
            }
            None => {
                if self.frames == 0 && !self.warned && self.started.is_some_and(|t| t.elapsed().as_secs() >= 3) {
                    self.warned = true;
                    eprintln!("[hal-camera] {}: no frame 3 s after streaming started (busy, unplugged or the device stalls)", self.path);
                }
            }
        }
        frame
    }
    fn poll_frame(&mut self) -> Option<Vec<u8>> {
        unsafe {
            let mut b: v4l2_buffer = std::mem::zeroed();
            b.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            b.memory = V4L2_MEMORY_MMAP;
            if ioctl(self.fd, VIDIOC_DQBUF, (&mut b as *mut v4l2_buffer).cast::<std::ffi::c_void>()) < 0 {
                return None;
            }
            // Preserve native latest-frame semantics when decode or a slow consumer made
            // capture fall behind: return older mmap buffers before copying the newest one.
            loop {
                let mut newer: v4l2_buffer = std::mem::zeroed();
                newer.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                newer.memory = V4L2_MEMORY_MMAP;
                if ioctl(
                    self.fd,
                    VIDIOC_DQBUF,
                    (&mut newer as *mut v4l2_buffer).cast::<std::ffi::c_void>(),
                ) < 0
                {
                    break;
                }
                ioctl(self.fd, VIDIOC_QBUF, (&mut b as *mut v4l2_buffer).cast::<std::ffi::c_void>());
                b = newer;
            }
            let result = (|| {
                let buffer = self.buffers.get(b.index as usize)?;
                if b.bytesused as usize > buffer.length || b.flags & 0x40 != 0 {
                    return None;
                }
                let raw = std::slice::from_raw_parts(buffer.ptr, b.bytesused as usize);
                let timestamp_ns = if b.flags & V4L2_BUF_FLAG_TIMESTAMP_MASK
                    == V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC
                {
                    (b.timestamp.tv_sec.max(0) as u64) * 1_000_000_000
                        + (b.timestamp.tv_usec.max(0) as u64) * 1000
                } else {
                    crate::clock_ns()
                };
                let f = &self.format;
                let w = f.width;
                let h = f.height;
                let stride = self.stride.max(w);
                let mut planes = Vec::new();
                let layout = match f.pixel {
                    VideoPixel::YUV420 => {
                        let y = (stride * h) as usize;
                        let cs = stride.div_ceil(2);
                        let c = (cs * h.div_ceil(2)) as usize;
                        planes.push(CameraPlane {
                            row_stride: stride,
                            pixel_stride: 1,
                            bytes: raw.get(..y)?,
                        });
                        planes.push(CameraPlane {
                            row_stride: cs,
                            pixel_stride: 1,
                            bytes: raw.get(y..y + c)?,
                        });
                        planes.push(CameraPlane {
                            row_stride: cs,
                            pixel_stride: 1,
                            bytes: raw.get(y + c..y + c * 2)?,
                        });
                        camera_layout::I420
                    }
                    VideoPixel::NV12 => {
                        let y = (stride * h) as usize;
                        let c = (stride * h.div_ceil(2)) as usize;
                        planes.push(CameraPlane {
                            row_stride: stride,
                            pixel_stride: 1,
                            bytes: raw.get(..y)?,
                        });
                        planes.push(CameraPlane {
                            row_stride: stride,
                            pixel_stride: 2,
                            bytes: raw.get(y..y + c)?,
                        });
                        camera_layout::NV12
                    }
                    VideoPixel::YUY2 => {
                        planes.push(CameraPlane {
                            row_stride: self.stride.max(w * 2),
                            pixel_stride: 2,
                            bytes: raw,
                        });
                        camera_layout::YUY2
                    }
                    VideoPixel::MJPEG => {
                        planes.push(CameraPlane {
                            row_stride: raw.len() as u32,
                            pixel_stride: 1,
                            bytes: raw,
                        });
                        camera_layout::MJPEG
                    }
                    VideoPixel::RGB24 => {
                        planes.push(CameraPlane {
                            row_stride: self.stride.max(w * 3),
                            pixel_stride: 3,
                            bytes: raw,
                        });
                        camera_layout::RGB24
                    }
                    VideoPixel::GRAY => {
                        planes.push(CameraPlane {
                            row_stride: stride,
                            pixel_stride: 1,
                            bytes: raw,
                        });
                        camera_layout::GRAY
                    }
                    VideoPixel::Unsupported(_) => {
                        planes.push(CameraPlane {
                            row_stride: self.stride,
                            pixel_stride: 1,
                            bytes: raw,
                        });
                        camera_layout::UNKNOWN
                    }
                };
                Some(pack_camera_frame(&CameraFrameData {
                    timestamp_ns,
                    width: w,
                    height: h,
                    layout,
                    matrix: if layout == camera_layout::MJPEG { 3 } else { 1 },
                    display_rotation: 0,
                    planes,
                }))
            })();
            if ioctl(self.fd, VIDIOC_QBUF, (&mut b as *mut v4l2_buffer).cast::<std::ffi::c_void>()) < 0 {
                eprintln!(
                    "[hal-camera] requeue {}: {}",
                    self.path,
                    std::io::Error::last_os_error()
                );
            }
            result
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            if self.streaming {
                let mut kind = V4L2_BUF_TYPE_VIDEO_CAPTURE;
                ioctl(self.fd, VIDIOC_STREAMOFF, (&mut kind as *mut u32).cast::<std::ffi::c_void>());
            }
            for b in &self.buffers {
                libc_sys::munmap(b.ptr.cast(), b.length);
            }
            libc_sys::close(self.fd);
        }
    }
}
#[derive(Default)]
pub struct Camera {
    devices: Vec<Device>,
    captures: HashMap<(u64, u64), Capture>,
    /// the last open error of each wanted camera, so a retry logs only a change
    failed: HashMap<(u64, u64), String>,
    /// open captures of the synthetic camera (`MAKEPAD_HAL_FAKE_CAMERA`), if one is listed
    fakes: HashMap<(u64, u64), FakeCapture>,
}
/// The synthetic camera the environment asks for, read once.
fn fake_desc() -> Option<&'static VideoInputInfo> {
    static FAKE: std::sync::OnceLock<Option<VideoInputInfo>> = std::sync::OnceLock::new();
    FAKE.get_or_init(fake_camera::listed).as_ref()
}
impl Camera {
    pub fn active(&self) -> bool {
        !self.captures.is_empty() || !self.fakes.is_empty()
    }
    pub fn cadence(&self) -> Option<std::time::Duration> {
        self.captures
            .values()
            .map(|c| &c.format)
            .chain(self.fakes.values().map(|c| c.format()))
            .filter_map(|f| f.frame_rate)
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .map(|fps| std::time::Duration::from_secs_f64(0.5 / fps))
            .min()
    }
    pub fn enumerate(&mut self) -> Vec<VideoInputInfo> {
        let mut paths: Vec<_> = std::fs::read_dir("/dev")
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .strip_prefix("video")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            })
            .map(|e| e.path())
            .collect();
        paths.sort();
        if paths.len() == self.devices.len()
            && paths
                .iter()
                .zip(&self.devices)
                .all(|(p, d)| p == std::path::Path::new(&d.path))
        {
            return self.devices.iter().map(|d| d.desc.clone()).chain(fake_desc().cloned()).collect();
        }
        self.devices = paths
            .iter()
            .filter_map(|p| Self::probe_device(&p.to_string_lossy()))
            .collect();
        self.captures
            .retain(|(input, _), _| self.devices.iter().any(|d| d.desc.id == *input));
        self.devices.iter().map(|d| d.desc.clone()).chain(fake_desc().cloned()).collect()
    }
    /// A wanted camera is listed but not open (its open failed: a device that is still busy from
    /// the last close, say). The worker tries again at its next housekeeping.
    pub fn unserved(&self, wanted: &[(u64, u64)]) -> bool {
        wanted.iter().any(|key| {
            !self.captures.contains_key(key)
                && !self.fakes.contains_key(key)
                && self.devices.iter().map(|d| &d.desc).chain(fake_desc()).any(|d| d.id == key.0 && d.formats.iter().any(|f| f.id == key.1))
        })
    }
    pub fn use_inputs(&mut self, inputs: &[(u64, u64)]) {
        for key in self.captures.keys().filter(|k| !inputs.contains(k)) {
            if let Some(c) = self.captures.get(key) {
                eprintln!("[hal-camera] {}: released ({} frames delivered)", c.path, c.frames);
            }
        }
        self.captures.retain(|key, _| inputs.contains(key));
        self.fakes.retain(|key, _| inputs.contains(key));
        self.failed.retain(|key, _| inputs.contains(key));
        for &(input, format) in inputs {
            if self.captures.contains_key(&(input, format)) || self.fakes.contains_key(&(input, format)) {
                continue;
            }
            if let Some(fake) = fake_desc().filter(|d| d.id == input) {
                if let Some(info) = fake.formats.iter().find(|f| f.id == format) {
                    eprintln!("[hal-camera] fake \"{}\": streaming {}x{} {:?} {:?} fps", fake.name, info.width, info.height, info.pixel, info.frame_rate);
                    self.fakes.insert((input, format), FakeCapture::open(&fake.name, info.clone()));
                }
                continue;
            }
            let Some(device) = self.devices.iter().find(|d| d.desc.id == input) else {
                continue;
            };
            let Some(format_info) = device.desc.formats.iter().find(|f| f.id == format) else {
                continue;
            };
            match Capture::open(&device.path, format_info.clone()).and_then(|mut c| {
                c.start()?;
                Ok(c)
            }) {
                Ok(c) => {
                    eprintln!(
                        "[hal-camera] {} \"{}\": streaming {}x{} {:?} {:?} fps (asked {}x{} {:?}), {} buffers",
                        device.path,
                        device.desc.name,
                        c.format.width,
                        c.format.height,
                        c.format.pixel,
                        c.format.frame_rate,
                        format_info.width,
                        format_info.height,
                        format_info.pixel,
                        c.buffers.len()
                    );
                    self.captures.insert((input, format), c);
                }
                Err(e) => {
                    if self.failed.get(&(input, format)) != Some(&e) {
                        eprintln!(
                            "[hal-camera] {} \"{}\": cannot open at {}x{} {:?}: {e} (tried again at every housekeeping until it opens or is no longer wanted)",
                            device.path, device.desc.name, format_info.width, format_info.height, format_info.pixel
                        );
                        self.failed.insert((input, format), e);
                    }
                }
            }
        }
    }
    pub fn format(&self, key: (u64, u64)) -> Option<&VideoFormatInfo> {
        self.captures.get(&key).map(|c| &c.format).or_else(|| self.fakes.get(&key).map(|c| c.format()))
    }
    pub fn poll(&mut self) -> Vec<((u64, u64), Vec<u8>)> {
        let fakes = self.fakes.iter_mut().filter_map(|(key, c)| c.poll().map(|p| (*key, p)));
        self.captures
            .iter_mut()
            .filter_map(|(key, c)| c.poll().map(|p| (*key, p)))
            .chain(fakes)
            .collect()
    }
    fn probe_device(path: &str) -> Option<Device> {
        unsafe {
            let path_cstr = std::ffi::CString::new(path).ok()?;
            let fd = libc_sys::open(
                path_cstr.as_ptr() as *const _,
                libc_sys::O_RDWR | libc_sys::O_NONBLOCK,
            );
            if fd < 0 {
                return None;
            }

            let result = Self::probe_device_fd(fd, path);
            libc_sys::close(fd);
            result
        }
    }

    unsafe fn probe_device_fd(fd: c_int, path: &str) -> Option<Device> {
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

        let name = cstr_from_bytes(&cap.card);
        if let Err(why) = judge_node(&name, caps, None) {
            eprintln!("[hal-camera] {path} \"{name}\": skipped, {why}");
            return None;
        }

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
                                let format_id = name_hash(&format!(
                                    "{} {} {:?} {}",
                                    width, height, pixel_format, frame_rate
                                ));
                                formats.push(VideoFormatInfo {
                                    id: format_id,
                                    width: width as u32,
                                    height: height as u32,
                                    pixel: pixel_format,
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
                            name_hash(&format!("{} {} {:?}", width, height, pixel_format));
                        formats.push(VideoFormatInfo {
                            id: format_id,
                            width: width as u32,
                            height: height as u32,
                            pixel: pixel_format,
                            frame_rate: None,
                        });
                    }
                }

                size_index += 1;
            }

            fmt_index += 1;
        }

        if let Err(why) = judge_node(&name, caps, Some(&formats)) {
            eprintln!("[hal-camera] {path} \"{name}\": skipped, {why}");
            return None;
        }

        let input_id = name_hash(path);
        eprintln!(
            "[hal-camera] {path} \"{name}\": camera, {} formats ({})",
            formats.len(),
            summary(&formats)
        );
        Some(Device {
            path: path.to_string(),
            desc: VideoInputInfo {
                id: input_id,
                name,
                formats,
                sensor_orientation: 0,
            },
        })
    }
}
/// Whether a V4L2 node is a camera a deck can show. `formats` is None while only the capabilities
/// are known. Metadata nodes (an OBSBOT's second node), output nodes and infrared sensors (an
/// ASUS "IR webcam", greyscale only) are not cameras; the first camera must never be one of them.
pub(crate) fn judge_node(name: &str, caps: u32, formats: Option<&[VideoFormatInfo]>) -> Result<(), String> {
    if caps & V4L2_CAP_VIDEO_CAPTURE == 0 {
        return Err(if caps & V4L2_CAP_META_CAPTURE != 0 {
            "a metadata node".into()
        } else {
            format!("not a video capture node (caps {caps:#x})")
        });
    }
    if name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word.eq_ignore_ascii_case("ir"))
    {
        return Err("an infrared camera".into());
    }
    if let Some(formats) = formats {
        let colour = |f: &VideoFormatInfo| {
            matches!(
                f.pixel,
                VideoPixel::YUY2
                    | VideoPixel::MJPEG
                    | VideoPixel::NV12
                    | VideoPixel::YUV420
                    | VideoPixel::RGB24
            )
        };
        if formats.is_empty() {
            return Err("it lists no frame sizes".into());
        }
        if !formats.iter().any(colour) {
            return Err("it offers no colour format (infrared or depth)".into());
        }
    }
    Ok(())
}

fn summary(formats: &[VideoFormatInfo]) -> String {
    let mut kinds: Vec<String> = Vec::new();
    for f in formats {
        let k = format!("{:?}", f.pixel);
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    let best = formats.iter().max_by_key(|f| f.width * f.height);
    match best {
        Some(b) => format!("{}; up to {}x{}", kinds.join("/"), b.width, b.height),
        None => kinds.join("/"),
    }
}

fn fourcc_to_video_pixel_format(fourcc: u32) -> VideoPixel {
    match fourcc {
        V4L2_PIX_FMT_YUYV => VideoPixel::YUY2,
        V4L2_PIX_FMT_MJPEG => VideoPixel::MJPEG,
        V4L2_PIX_FMT_NV12 => VideoPixel::NV12,
        V4L2_PIX_FMT_YUV420 => VideoPixel::YUV420,
        V4L2_PIX_FMT_RGB24 => VideoPixel::RGB24,
        V4L2_PIX_FMT_GREY => VideoPixel::GRAY,
        other => VideoPixel::Unsupported(other),
    }
}

fn video_pixel_format_to_fourcc(format: VideoPixel) -> u32 {
    match format {
        VideoPixel::YUY2 => V4L2_PIX_FMT_YUYV,
        VideoPixel::MJPEG => V4L2_PIX_FMT_MJPEG,
        VideoPixel::NV12 => V4L2_PIX_FMT_NV12,
        VideoPixel::YUV420 => V4L2_PIX_FMT_YUV420,
        VideoPixel::RGB24 => V4L2_PIX_FMT_RGB24,
        VideoPixel::GRAY => V4L2_PIX_FMT_GREY,
        VideoPixel::Unsupported(fcc) => fcc,
    }
}

unsafe fn cstr_from_bytes(bytes: &[u8]) -> String {
    // UTF-8 Lossy: V4L2 device names are kernel byte arrays for display only
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURE: u32 = V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_STREAMING | V4L2_CAP_DEVICE_CAPS;
    const META: u32 = V4L2_CAP_META_CAPTURE | V4L2_CAP_STREAMING | V4L2_CAP_DEVICE_CAPS;

    fn fmt(pixel: VideoPixel, w: u32, h: u32) -> VideoFormatInfo {
        VideoFormatInfo { id: 1, width: w, height: h, frame_rate: Some(30.0), pixel }
    }

    /// The nodes of node 149, with the card strings v4l2 returns (31 characters at most).
    fn nodes() -> Vec<(&'static str, &'static str, u32, Vec<VideoFormatInfo>)> {
        let colour = vec![fmt(VideoPixel::MJPEG, 1920, 1080), fmt(VideoPixel::YUY2, 640, 480)];
        let grey = vec![fmt(VideoPixel::GRAY, 640, 360), fmt(VideoPixel::Unsupported(0x20363159), 640, 360)];
        vec![
            ("/dev/video0", "ASUS FHD webcam: ASUS FHD webca", CAPTURE, colour.clone()),
            ("/dev/video1", "ASUS FHD webcam: ASUS FHD webca", META, vec![]),
            ("/dev/video2", "ASUS IR webcam: ASUS IR webcam", CAPTURE, grey),
            ("/dev/video3", "ASUS IR webcam: ASUS IR webcam", META, vec![]),
            ("/dev/video4", "OBSBOT Meet 2: OBSBOT Meet 2 St", CAPTURE, colour),
            ("/dev/video5", "OBSBOT Meet 2: OBSBOT Meet 2 St", META, vec![]),
        ]
    }

    fn listed() -> Vec<(&'static str, &'static str)> {
        nodes()
            .into_iter()
            .filter(|(_, name, caps, formats)| {
                judge_node(name, *caps, None).is_ok() && judge_node(name, *caps, Some(formats)).is_ok()
            })
            .map(|(path, name, _, _)| (path, name))
            .collect()
    }

    #[test]
    fn only_real_colour_capture_nodes_are_listed() {
        assert_eq!(
            listed().iter().map(|(p, _)| *p).collect::<Vec<_>>(),
            ["/dev/video0", "/dev/video4"]
        );
    }

    #[test]
    fn the_metadata_and_infrared_nodes_say_why() {
        assert!(judge_node("OBSBOT Meet 2: OBSBOT Meet 2 St", META, None).unwrap_err().contains("metadata"));
        assert!(judge_node("ASUS IR webcam: ASUS IR webcam", CAPTURE, None).unwrap_err().contains("infrared"));
        let grey = [fmt(VideoPixel::GRAY, 640, 360)];
        assert!(judge_node("Depth", CAPTURE, Some(&grey)).unwrap_err().contains("colour"));
        // "IR" is a word, not a substring: "Mirror cam" and "Fire" are cameras
        assert!(judge_node("Mirror cam", CAPTURE, None).is_ok());
    }

    #[test]
    fn the_slide_names_pick_the_obsbot_or_the_first_asus() {
        let names: Vec<String> = listed().iter().map(|(_, n)| n.to_lowercase()).collect();
        let by = |wanted: &str| names.iter().position(|n| n.contains(&wanted.to_lowercase()));
        assert_eq!(listed()[by("OBSBOT").unwrap()].0, "/dev/video4");
        assert_eq!(listed()[0].0, "/dev/video0");
    }

    #[test]
    fn fourcc_round_trips() {
        for p in [VideoPixel::YUY2, VideoPixel::MJPEG, VideoPixel::NV12, VideoPixel::YUV420, VideoPixel::RGB24, VideoPixel::GRAY] {
            assert_eq!(fourcc_to_video_pixel_format(video_pixel_format_to_fourcc(p)), p);
        }
    }
}
