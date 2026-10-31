//! Other processes' GPU buffers (a Wayland compositor's client buffers) as textures.
//!
//! The host asks what the GPU can import, imports each buffer once into a `Texture`, and reports
//! the buffer's lifetime through fences: the writer's fence is waited on (by the caller, with
//! CPU polling) before `foreign_dmabuf_acquire`, and `Cx::frame_completion` asynchronously publishes a sync_file
//! covering every use submitted so far, for the writer to wait on before it reuses the buffer.
//! Only Vulkan renderers import; other backends report no support.
use std::os::fd::OwnedFd;

pub struct ForeignDmabufPlane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

/// One dma-buf image: DRM fourcc (AR24 or XR24), layout modifier, a single plane.
pub struct ForeignDmabuf {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<ForeignDmabufPlane>,
}

pub struct ForeignDmabufSupport {
    /// dev_t of the GPU's render node.
    pub render_device: u64,
    pub render_node: std::path::PathBuf,
    /// Importable (fourcc, modifier) pairs.
    pub formats: Vec<(u32, u64)>,
}

