//! V4L2 mmap capture. Device setup, frame copies and teardown run on the media worker.
use super::{libc_sys, v4l2_sys::*};
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
            Ok(())
        }
    }
    fn poll(&mut self) -> Option<Vec<u8>> {
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
}
impl Camera {
    pub fn active(&self) -> bool {
        !self.captures.is_empty()
    }
    pub fn cadence(&self) -> Option<std::time::Duration> {
        self.captures
            .values()
            .filter_map(|c| c.format.frame_rate)
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
            return self.devices.iter().map(|d| d.desc.clone()).collect();
        }
        self.devices = paths
            .iter()
            .filter_map(|p| Self::probe_device(&p.to_string_lossy()))
            .collect();
        self.captures
            .retain(|(input, _), _| self.devices.iter().any(|d| d.desc.id == *input));
        self.devices.iter().map(|d| d.desc.clone()).collect()
    }
    pub fn use_inputs(&mut self, inputs: &[(u64, u64)]) {
        self.captures.retain(|key, _| inputs.contains(key));
        for &(input, format) in inputs {
            if self.captures.contains_key(&(input, format)) {
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
                    self.captures.insert((input, format), c);
                }
                Err(e) => eprintln!("[hal-camera] {e}"),
            }
        }
    }
    pub fn format(&self, key: (u64, u64)) -> Option<&VideoFormatInfo> {
        self.captures.get(&key).map(|c| &c.format)
    }
    pub fn poll(&mut self) -> Vec<((u64, u64), Vec<u8>)> {
        self.captures
            .iter_mut()
            .filter_map(|(key, c)| c.poll().map(|p| (*key, p)))
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

        if formats.is_empty() {
            return None;
        }

        let input_id = name_hash(path);
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
