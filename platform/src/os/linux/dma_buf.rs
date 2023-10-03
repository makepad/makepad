#![allow(dead_code)]
//! # Linux cross-process DMA-BUF-based image ("texture") sharing
//!
//! An [`Image<FD>`] primarily contains [DMA-BUF] (`FD`-typed) file descriptor(s)
//! (within each [`ImagePlane<FD>`], which also tracks its buffer's "2D slice"),
//! and the ["DRM format"] ([`DrmFormat`]) describing the image's texel encoding,
//! all combined into a conveniently (de)serializable form (as long as `FD` is).
//!
//! ---
//!
//! Under EGL, this allows sharing an OpenGL texture across processes, e.g.:
//! * A creates an `EGLImage` from some OpenGL texture (or another resource)
//! * A exports its `EGLImage` using [`EGL_MESA_image_dma_buf_export`]
//! * A passes to B its [DMA-BUF] file descriptor(s) and ["DRM format"] metadata
//! * B imports it as an `EGLImage` using [`EGL_EXT_image_dma_buf_import`]
//! * B exposes its `EGLImage` as an OpenGL texture using [`glEGLImageTargetTexture2DOES`]
//!
//! [DMA-BUF]: https://docs.kernel.org/driver-api/dma-buf.html
//! ["DRM format"]: https://docs.kernel.org/gpu/drm-kms.html#drm-format-handling
//! [`EGL_MESA_image_dma_buf_export`]: https://registry.khronos.org/EGL/extensions/MESA/EGL_MESA_image_dma_buf_export.txt
//! [`EGL_EXT_image_dma_buf_import`]: https://registry.khronos.org/EGL/extensions/EXT/EGL_EXT_image_dma_buf_import.txt
//! [`glEGLImageTargetTexture2DOES`]: https://registry.khronos.org/OpenGL/extensions/OES/OES_EGL_image.txt

use crate::makepad_micro_serde::*;

#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
pub struct Image<FD>
    // HACK(eddyb) hint `{Ser,De}{Bin,Json}` derivers to add their own bounds.
    where FD: Sized
{
    pub drm_format: DrmFormat,
    // FIXME(eddyb) support 2-4 planes (not needed for RGBA, so most likely only
    // relevant to YUV video decode streams - or certain forms of compression).
    pub planes: ImagePlane<FD>,
}

impl<FD> Image<FD> {
    pub fn planes_fd_map<FD2>(self, f: impl FnMut(FD) -> FD2) -> Image<FD2> {
        let Image { drm_format, planes: plane0 } = self;
        Image { drm_format, planes: plane0.fd_map(f) }
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

#[derive(Copy, Clone, Debug, PartialEq, SerBin, DeBin, SerJson, DeJson)]
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
    fn fd_map<FD2>(self, f: impl FnOnce(FD) -> FD2) -> ImagePlane<FD2> {
        let ImagePlane { dma_buf_fd, offset, stride } = self;
        ImagePlane { dma_buf_fd: f(dma_buf_fd), offset, stride }
    }
}
