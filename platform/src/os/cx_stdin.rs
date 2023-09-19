#![allow(dead_code)]
use {
    std::cell::Cell,
    crate::{
        cx::Cx,
        cursor::MouseCursor,
        makepad_micro_serde::*,
        makepad_math::dvec2,
        window::CxWindowPool,
        area::Area,
        event::{
            KeyEvent,
            ScrollEvent,
            MouseDownEvent,
            MouseUpEvent,
            MouseMoveEvent,
        }
    }
};

#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct StdinWindowSize {
    pub width: f64,
    pub height: f64,
    pub dpi_factor: f64,
    
    #[cfg(target_os = "macos")]
    pub swapchain_front: u32,  // DX11 handles or XPC hashmap indices
    
    #[cfg(target_os = "macos")]
    pub swapchain_handle: u64,  // DX11 handles or XPC hashmap indices
    
    #[cfg(target_os = "windows")]
    pub swapchain_handles: [u64; 2],  // DX11 handles or XPC hashmap indices

    // FIXME(eddyb) double-buffering support is intentionally left out, as it's
    // being added to other OSes, this just uses the same name as that other work.
    #[cfg(target_os = "linux")]
    pub swapchain_handles: [linux_dma_buf::Image<linux_dma_buf::RemoteFd>; 2],
}

// FIXME(eddyb) move this into `os::linux` somewhere.
#[cfg(target_os = "linux")]
pub mod linux_dma_buf {
    use crate::makepad_micro_serde::*;

    use std::os::{self, fd::AsRawFd as _};

    #[derive(Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
    pub struct Image<FD>
        // HACK(eddyb) hint `{Ser,De}{Bin,Json}` derivers to add their own bounds.
        where FD: Sized
    {
        // FIXME(eddyb) are these redundant with the `StdinWindowSize` size?
        pub width: u32,
        pub height: u32,

        pub drm_format: DrmFormat,
        // FIXME(eddyb) support 2-4 planes (not needed for RGBA, so most likely only
        // relevant to YUV video decode streams - or certain forms of compression).
        pub planes: ImagePlane<FD>,
    }

    impl<FD> Image<FD> {
        pub fn planes_map<FD2>(self, f: impl Fn(ImagePlane<FD>) -> ImagePlane<FD2>) -> Image<FD2> {
            let Image { width, height, drm_format, planes: plane0 } = self;
            Image { width, height, drm_format, planes: f(plane0) }
        }
        pub fn planes_ref_map<FD2>(&self, f: impl Fn(&ImagePlane<FD>) -> ImagePlane<FD2>) -> Image<FD2> {
            let Image { width, height, drm_format, planes: ref plane0 } = *self;
            Image { width, height, drm_format, planes: f(plane0) }
        }
    }

    impl<FD> Copy for Image<FD> where ImagePlane<FD>: Copy {}
    impl<FD> Clone for Image<FD> where ImagePlane<FD>: Clone {
        fn clone(&self) -> Self {
            self.planes_ref_map(|plane| plane.clone())
        }
    }

    impl Image<os::fd::OwnedFd> {
        pub fn as_remote(&self) -> Image<RemoteFd> {
            self.planes_ref_map(|plane| plane.as_remote())
        }
    }

    /// In the Linux DRM+KMS system (i.e. kernel-side GPU drivers), a "DRM format"
    /// is an image format (i.e. a specific byte-level encoding of texel data)
    /// that framebuffers (or more generally "surfaces" / "images") could use,
    /// provided that all the GPUs involved support the specific format used.
    ///
    /// See also <https://docs.kernel.org/gpu/drm-kms.html#drm-format-handling>.
    #[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
    pub struct DrmFormat {
        /// FourCC code for a "DRM format", i.e. one of the `DRM_FORMAT_*` values
        /// defined in `drm/drm_fourcc.h`, and the main aspect of a "DRM format"
        /// that userspace needs to care about (e.g. RGB vs YUV, bit width, etc.).
        ///
        /// For example, non-HDR RGBA surfaces will almost always use the format
        /// `DRM_FORMAT_ABGR8888` (with FourCC `"AB24"`, i.e. `0x34324241`), and:
        /// - "A" can be replaced with "X" (disabling the alpha channel)
        /// - "AB" can be reversed, to get "BA" (ABGR -> BGRA)
        /// - "B" can be replaced with "R" (ABGR -> ARGB)
        /// - "AR" can be reversed, to get "RA" (ARGB -> RGBA)
        /// - "24" can be replaced with "30" or "48" (increasing bits per channel)
        ///
        /// Some formats also require multiple "planes" (i.e. independent buffers),
        /// and while that's commonly for YUV formats, planar RGBA also exists.
        pub fourcc: u32,

        /// Each "DRM format" may be further "modified" with additional features,
        /// describing how memory is accessed by GPU texture units (e.g. "tiling"),
        /// and optionally requiring additional "planes" for compression purposes.
        ///
        /// To userspace, the modifiers are almost always opaque and merely need to
        /// be passed from an image exporter to an image importer, to correctly
        /// interpret the GPU memory in the same way on both sides.
        pub modifiers: u64,
    }

    #[derive(Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
    pub struct ImagePlane<FD>
        // HACK(eddyb) hint `{Ser,De}{Bin,Json}` derivers to add their own bounds.
        where FD: Sized
    {
        /// Linux DMA-BUF file descriptor, representing a generic GPU buffer object.
        ///
        /// See also <https://docs.kernel.org/driver-api/dma-buf.html>.
        pub dma_buf_fd: FD,

        /// This plane's starting position (in bytes), in the DMA-BUF buffer.
        pub offset: u32,

        /// This plane's stride (aka "pitch") for texel rows, in the DMA-BUF buffer.
        pub stride: u32,
    }

    impl<FD> ImagePlane<FD> {
        pub fn fd_as_ref(&self) -> ImagePlane<&FD> {
            let ImagePlane { ref dma_buf_fd, offset, stride } = *self;
            ImagePlane { dma_buf_fd, offset, stride }
        }
        pub fn fd_map<FD2>(self, f: impl FnOnce(FD) -> FD2) -> ImagePlane<FD2> {
            let ImagePlane { dma_buf_fd, offset, stride } = self;
            ImagePlane { dma_buf_fd: f(dma_buf_fd), offset, stride }
        }
    }

    impl Copy for ImagePlane<RemoteFd> {}
    impl Clone for ImagePlane<RemoteFd> {
        fn clone(&self) -> Self {
            self.fd_as_ref().fd_map(|&fd| fd)
        }
    }

    impl Clone for ImagePlane<os::fd::OwnedFd> {
        fn clone(&self) -> Self {
            self.fd_as_ref().fd_map(|fd| fd.try_clone().unwrap())
        }
    }

    impl ImagePlane<os::fd::OwnedFd> {
        pub fn as_remote(&self) -> ImagePlane<RemoteFd> {
            self.fd_as_ref().fd_map(|fd| RemoteFd {
                remote_pid: std::process::id(),
                remote_fd: fd.as_raw_fd(),
            })
        }
    }

    // HACK(eddyb) to avoid needing an UNIX domain socket, we pass file descriptors
    // to child processes via `pidfd_getfd` (see also `linux_x11_stdin::pid_fd`).
    #[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
    pub struct RemoteFd {
        pub remote_pid: u32,
        pub remote_fd: i32,
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseDown{
   pub button: usize,
   pub x: f64,
   pub y: f64,
   pub time: f64,
}

impl From<StdinMouseDown> for MouseDownEvent {
    fn from(v: StdinMouseDown) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            button: v.button,
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
            handled: Cell::new(Area::Empty),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseMove{
   pub time: f64,
   pub x: f64,
   pub y: f64
}

impl From<StdinMouseMove> for MouseMoveEvent {
    fn from(v: StdinMouseMove) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
            handled: Cell::new(Area::Empty),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinMouseUp{
   pub time: f64,
   pub button: usize,
   pub x: f64,
   pub y: f64
}

impl From<StdinMouseUp> for MouseUpEvent {
    fn from(v: StdinMouseUp) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            button: v.button,
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            time: v.time,
        }
    }
}


#[derive(Clone, Copy, Debug, Default, SerBin, DeBin, SerJson, DeJson, PartialEq)]
pub struct StdinScroll{
   pub time: f64,
   pub sx: f64,
   pub sy: f64,
   pub x: f64,
   pub y: f64,
   pub is_mouse: bool,
}

impl From<StdinScroll> for ScrollEvent {
    fn from(v: StdinScroll) -> Self {
        Self{
            abs: dvec2(v.x, v.y),
            scroll: dvec2(v.sx, v.sy),
            window_id: CxWindowPool::id_zero(),
            modifiers: Default::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: v.is_mouse,
            time: v.time,
        }
    }
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum HostToStdin{
    WindowSize(StdinWindowSize),
    Tick{
        buffer_id: u64,
        frame: u64,
        time: f64,
    },
    MouseDown(StdinMouseDown),
    MouseUp(StdinMouseUp),
    MouseMove(StdinMouseMove),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    Scroll(StdinScroll),
    ReloadFile{
        file:String,
        contents:String
    },
}

#[derive(Clone, Debug, SerBin, DeBin, SerJson, DeJson)]
pub enum StdinToHost{
    ReadyToStart,
    SetCursor(MouseCursor),
    DrawCompleteAndFlip(usize),  // the client is done drawing, and the texture is completely updated
}

impl StdinToHost{
    pub fn to_json(&self)->String{
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl HostToStdin{
    pub fn to_json(&self)->String{
        let mut json = self.serialize_json();
        json.push('\n');
        json
    }
}

impl Cx {
    
}
