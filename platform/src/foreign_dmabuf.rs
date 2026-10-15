//! Other processes' GPU buffers (a Wayland compositor's client buffers) as textures.
//!
//! The host asks what the GPU can import, imports each buffer once into a `Texture`, and reports
//! the buffer's lifetime through fences: the writer's fence is waited on (by the caller, with
//! CPU polling) before `foreign_dmabuf_acquire`, and `foreign_dmabuf_fence` asynchronously publishes a sync_file
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

/// A nonblocking release result. Pending is distinct from a signaled fence (no FD).
/// The compositor must retain the buffer on failure; an error is never permission to write.
pub struct ForeignDmabufFence {
    /// the HAL's number for it (`makepad_hal::dmabuf::hal_dmabuf_fence`); 0 once it answered
    fence: u64,
}
impl ForeignDmabufFence {
    pub(crate) fn from_hal(fence: u64) -> Self { Self { fence } }
    pub fn poll(&mut self) -> Option<Result<Option<OwnedFd>, String>> {
        use std::os::fd::FromRawFd;
        if self.fence == 0 {
            return Some(Err("the DMA-BUF release fence answered already".into()));
        }
        let mut fd = -1;
        let answer = match makepad_hal::dmabuf::hal_dmabuf_fence_poll(self.fence, &mut fd) {
            0 => return None,
            1 => Ok((fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })),
            _ => Err(crate::os::hal::linux::dmabuf_error()),
        };
        self.fence = 0;
        Some(answer)
    }
}
impl Drop for ForeignDmabufFence {
    fn drop(&mut self) {
        if self.fence != 0 {
            makepad_hal::dmabuf::hal_dmabuf_fence_drop(self.fence);
        }
    }
}
