//! The descriptor channel beside a hosted app's hub socket (Linux): the host sends each shared
//! swapchain image's DMA-BUF (or OPAQUE_FD memory) and its timeline semaphore over a Unix
//! socket, ahead of the `StudioToApp::Swapchain` message that describes them. Used by both
//! hosted clients -- Cx's (a native app the WM hosts) and the HAL's WM-hosted shell -- and by
//! hosts (the WM, Stage) for the sending side.

use crate::dmabuf::{DmaBuf, OpaqueMemory};
use crate::linux_ipc::{self, Channel, FixedSizeEncoding};
use makepad_studio_protocol::{PresentableImageId, SharedPresentableImage};
use std::io;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::path::PathBuf;

/// Host to client: an image id and one of its descriptors.
pub type H2C = (PresentableImageId, OwnedFd);
pub type C2H = linux_ipc::Never;
pub type HostEndpoint = Channel<H2C, C2H>;
pub type ClientEndpoint = Channel<C2H, H2C>;

impl FixedSizeEncoding<{ u64::BYTE_LEN }, 0> for PresentableImageId {
    fn encode(&self) -> ([u8; Self::BYTE_LEN], [BorrowedFd<'_>; 0]) {
        let (bytes, []) = self.as_u64().encode();
        (bytes, [])
    }
    fn decode(bytes: [u8; Self::BYTE_LEN], fds: [OwnedFd; 0]) -> Self {
        Self::from_u64(u64::decode(bytes, fds))
    }
}

/// Where the host of hub `port` listens for client `client`'s descriptor channel.
pub fn aux_path(port: u16, client: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/makepad-stdin-aux-{port}-{client}.sock"))
}

fn other(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, error)
}

/// Receive `image`'s descriptors (in the host's send order: memory, then the Vulkan timeline or
/// the DMA-BUF timeline) and make the import descriptor of an `width` x `height` allocation.
/// Descriptors of older images still in the channel (a replaced swapchain) are dropped.
pub fn recv_image(endpoint: &ClientEndpoint, image: SharedPresentableImage, width: u32, height: u32) -> io::Result<DmaBuf> {
    let id = image.id;
    let shared = image.image;
    if shared.vulkan.is_some() && shared.dma_buf_timeline {
        return Err(other("ambiguous hosted image transport"));
    }
    let mut stale = 0usize;
    let memory = loop {
        let (got, fd) = endpoint.recv()?;
        if got == id {
            break fd;
        }
        stale += 1;
        if stale >= 64 {
            return Err(other(format!("hosted image descriptors: expected {id:?}, last got {got:?}, dropped {stale}")));
        }
    };
    let next = |what: &str| -> io::Result<OwnedFd> {
        let (got, fd) = endpoint.recv()?;
        if got != id {
            return Err(other(format!("{what} does not match image {id:?}")));
        }
        Ok(fd)
    };
    let (opaque, timeline_fd, allocation_size) = match shared.vulkan {
        Some(vulkan) => (
            Some(OpaqueMemory { memory_type_index: vulkan.memory_type_index, device_uuid: vulkan.device_uuid, driver_uuid: vulkan.driver_uuid }),
            Some(next("the Vulkan timeline")?),
            vulkan.allocation_size,
        ),
        None => {
            let timeline = if shared.dma_buf_timeline { Some(next("the DMA-BUF timeline")?) } else { None };
            (None, timeline, DmaBuf::size(&memory).map_err(other)?)
        }
    };
    if timeline_fd.is_none() {
        return Err(other("hosted image needs an explicit writer/read lease timeline"));
    }
    Ok(DmaBuf {
        fd: memory,
        width,
        height,
        modifier: shared.drm_format.modifiers,
        offset: shared.plane.offset as u64,
        row_pitch: shared.plane.stride as u64,
        allocation_size,
        fourcc: if opaque.is_some() { u32::from_le_bytes(*b"AR24") } else { shared.drm_format.fourcc },
        sample_only: false,
        opaque,
        timeline_fd,
    })
}
