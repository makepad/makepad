#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_long, c_short, c_ulong, c_void};

// ioctl direction bits
const IOC_NONE: u32 = 0;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;

const fn ioc(dir: u32, ty: u32, nr: u32, size: usize) -> c_ulong {
    ((dir as c_ulong) << 30) | ((ty as c_ulong) << 8) | (nr as c_ulong) | ((size as c_ulong) << 16)
}

// V4L2 capabilities
pub const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x00000001;
pub const V4L2_CAP_STREAMING: u32 = 0x04000000;
pub const V4L2_CAP_DEVICE_CAPS: u32 = 0x80000000;

// Buffer types
pub const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;

// Memory types
pub const V4L2_MEMORY_MMAP: u32 = 1;

// Fields
pub const V4L2_FIELD_ANY: u32 = 0;

// Pixel formats (v4l2_fourcc)
const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

pub const V4L2_PIX_FMT_YUYV: u32 = fourcc(b'Y', b'U', b'Y', b'V');
pub const V4L2_PIX_FMT_MJPEG: u32 = fourcc(b'M', b'J', b'P', b'G');
pub const V4L2_PIX_FMT_NV12: u32 = fourcc(b'N', b'V', b'1', b'2');
pub const V4L2_PIX_FMT_YUV420: u32 = fourcc(b'Y', b'U', b'1', b'2');
pub const V4L2_PIX_FMT_RGB24: u32 = fourcc(b'R', b'G', b'B', b'3');
pub const V4L2_PIX_FMT_GREY: u32 = fourcc(b'G', b'R', b'E', b'Y');

// Frame size types
pub const V4L2_FRMSIZE_TYPE_DISCRETE: u32 = 1;

// Frame interval types
pub const V4L2_FRMIVAL_TYPE_DISCRETE: u32 = 1;

// Streamparm capability
pub const V4L2_CAP_TIMEPERFRAME: u32 = 0x1000;

// poll
pub const POLLIN: c_short = 0x0001;

// inotify
pub const IN_CREATE: u32 = 0x00000100;
pub const IN_DELETE: u32 = 0x00000200;
pub const IN_NONBLOCK: c_int = 0o4000;

// --- Structs ---

#[repr(C)]
pub struct v4l2_capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
pub struct v4l2_fmtdesc {
    pub index: u32,
    pub type_: u32,
    pub flags: u32,
    pub description: [u8; 32],
    pub pixelformat: u32,
    pub mbus_code: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_fract {
    pub numerator: u32,
    pub denominator: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_pix_format {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub bytesperline: u32,
    pub sizeimage: u32,
    pub colorspace: u32,
    pub priv_: u32,
}

// The C union in v4l2_format has pointer-aligned members (v4l2_window).
// Adding a pointer member forces correct alignment on both 32-bit and 64-bit.
#[repr(C)]
pub union v4l2_format_fmt {
    pub pix: v4l2_pix_format,
    pub raw_data: [u8; 200],
    _ptr_align: *const c_void,
}

#[repr(C)]
pub struct v4l2_format {
    pub type_: u32,
    pub fmt: v4l2_format_fmt,
}

#[repr(C)]
pub struct v4l2_requestbuffers {
    pub count: u32,
    pub type_: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub flags: u8,
    pub reserved: [u8; 3],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_timeval {
    pub tv_sec: c_long,
    pub tv_usec: c_long,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_timecode {
    pub type_: u32,
    pub flags: u32,
    pub frames: u8,
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub userbits: [u8; 4],
}

#[repr(C)]
pub union v4l2_buffer_m {
    pub offset: u32,
    pub userptr: c_ulong,
    pub planes: *mut c_void,
    pub fd: i32,
}

#[repr(C)]
pub struct v4l2_buffer {
    pub index: u32,
    pub type_: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: v4l2_timeval,
    pub timecode: v4l2_timecode,
    pub sequence: u32,
    pub memory: u32,
    pub m: v4l2_buffer_m,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_frmsize_discrete {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_frmsize_stepwise {
    pub min_width: u32,
    pub max_width: u32,
    pub step_width: u32,
    pub min_height: u32,
    pub max_height: u32,
    pub step_height: u32,
}

#[repr(C)]
pub union v4l2_frmsizeenum_u {
    pub discrete: v4l2_frmsize_discrete,
    pub stepwise: v4l2_frmsize_stepwise,
}

#[repr(C)]
pub struct v4l2_frmsizeenum {
    pub index: u32,
    pub pixel_format: u32,
    pub type_: u32,
    pub u: v4l2_frmsizeenum_u,
    pub reserved: [u32; 2],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_frmival_stepwise {
    pub min: v4l2_fract,
    pub max: v4l2_fract,
    pub step: v4l2_fract,
}

#[repr(C)]
pub union v4l2_frmivalenum_u {
    pub discrete: v4l2_fract,
    pub stepwise: v4l2_frmival_stepwise,
}

#[repr(C)]
pub struct v4l2_frmivalenum {
    pub index: u32,
    pub pixel_format: u32,
    pub width: u32,
    pub height: u32,
    pub type_: u32,
    pub u: v4l2_frmivalenum_u,
    pub reserved: [u32; 2],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct v4l2_captureparm {
    pub capability: u32,
    pub capturemode: u32,
    pub timeperframe: v4l2_fract,
    pub extendedmode: u32,
    pub readbuffers: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
pub union v4l2_streamparm_parm {
    pub capture: v4l2_captureparm,
    pub raw_data: [u8; 200],
}

#[repr(C)]
pub struct v4l2_streamparm {
    pub type_: u32,
    pub parm: v4l2_streamparm_parm,
}

#[repr(C)]
pub struct pollfd {
    pub fd: c_int,
    pub events: c_short,
    pub revents: c_short,
}

#[repr(C)]
pub struct inotify_event {
    pub wd: c_int,
    pub mask: u32,
    pub cookie: u32,
    pub len: u32,
    // variable-length name follows
}

// --- ioctl constants ---

pub const VIDIOC_QUERYCAP: c_ulong = ioc(IOC_READ, 0x56, 0, std::mem::size_of::<v4l2_capability>());
pub const VIDIOC_ENUM_FMT: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    2,
    std::mem::size_of::<v4l2_fmtdesc>(),
);
pub const VIDIOC_G_FMT: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    4,
    std::mem::size_of::<v4l2_format>(),
);
pub const VIDIOC_S_FMT: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    5,
    std::mem::size_of::<v4l2_format>(),
);
pub const VIDIOC_REQBUFS: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    8,
    std::mem::size_of::<v4l2_requestbuffers>(),
);
pub const VIDIOC_QUERYBUF: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    9,
    std::mem::size_of::<v4l2_buffer>(),
);
pub const VIDIOC_QBUF: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    15,
    std::mem::size_of::<v4l2_buffer>(),
);
pub const VIDIOC_DQBUF: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    17,
    std::mem::size_of::<v4l2_buffer>(),
);
pub const VIDIOC_STREAMON: c_ulong = ioc(IOC_WRITE, 0x56, 18, std::mem::size_of::<c_int>());
pub const VIDIOC_STREAMOFF: c_ulong = ioc(IOC_WRITE, 0x56, 19, std::mem::size_of::<c_int>());
pub const VIDIOC_S_PARM: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    22,
    std::mem::size_of::<v4l2_streamparm>(),
);
pub const VIDIOC_ENUM_FRAMESIZES: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    74,
    std::mem::size_of::<v4l2_frmsizeenum>(),
);
pub const VIDIOC_ENUM_FRAMEINTERVALS: c_ulong = ioc(
    IOC_READ | IOC_WRITE,
    0x56,
    75,
    std::mem::size_of::<v4l2_frmivalenum>(),
);

// --- extern "C" ---

extern "C" {
    pub fn ioctl(fd: c_int, request: c_ulong, arg: *mut c_void) -> c_int;
    pub fn poll(fds: *mut pollfd, nfds: c_ulong, timeout: c_int) -> c_int;
    pub fn inotify_init1(flags: c_int) -> c_int;
    pub fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: u32) -> c_int;
}
