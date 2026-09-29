//! Screen-capture surfaces on the windowed Vulkan renderer (the backend half
//! of [`crate::screen_capture::add_screen_capture_surface`]).
//!
//! While a surface sink is due a frame, the window's command buffer copies
//! the swapchain image into the next free buffer of a small ring of
//! persistently mapped host-visible buffers, next to the frame's own work.
//! The frame is not waited for: the renderer already waits for the previous
//! submission's fence before it records the next frame, and that is where the
//! copied buffer is handed to the sinks. The render thread never reads the
//! pixels; the recorder's encoder thread does (and converts them there).
//!
//! A buffer is free when the ring holds its only handle; with none free the
//! frame is skipped, never waited for.

use {
    crate::screen_capture::{MappedCapture, ScreenCaptureSurface, SurfaceInner},
    ash::vk,
    std::cell::RefCell,
    std::sync::atomic::{AtomicU64, Ordering},
    std::sync::Arc,
};

/// Buffers in use at once: one being copied, one waiting for delivery, the
/// sink's pending slot and the one the encoder is reading.
const RING_BUFFERS: usize = 4;

/// Frames skipped because every ring buffer was in use. `MAKEPAD_TRACE=screencap`.
static SKIPPED: AtomicU64 = AtomicU64::new(0);

/// One ring buffer: host-visible, coherent, mapped for its whole life. Freed
/// when the last handle (the ring's, a sink's, the encoder's) goes.
pub(crate) struct CaptureBuffer {
    device: ash::Device,
    pub(crate) buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *const u8,
    len: usize,
    stride: usize,
    bgra: bool,
}

// The mapping is only read, and only after the GPU finished writing it.
unsafe impl Send for CaptureBuffer {}
unsafe impl Sync for CaptureBuffer {}

impl MappedCapture for CaptureBuffer {
    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.mapped, self.len) }
    }
    fn stride(&self) -> usize {
        self.stride
    }
    fn bgra(&self) -> bool {
        self.bgra
    }
}

impl Drop for CaptureBuffer {
    fn drop(&mut self) {
        unsafe {
            self.device.unmap_memory(self.memory);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

struct Ring {
    /// Swapchain extent and channel order the buffers were made for.
    key: (u32, u32, bool),
    buffers: Vec<Arc<CaptureBuffer>>,
    /// Copied in the last submission, delivered once its fence signalled.
    pending: Vec<(Option<usize>, Arc<CaptureBuffer>, u32, u32)>,
}

thread_local! {
    static RING: RefCell<Option<Ring>> = const { RefCell::new(None) };
}

/// A ring buffer to copy this frame into, if a surface sink of `window_id`
/// is due one and a buffer is free. `create` makes a host-visible,
/// coherent buffer of the given size (buffer, memory). The caller records
/// the copy of the whole `width` x `height` image, tightly packed, and then
/// calls [`queue`] with the result.
pub(crate) fn begin(
    device: &ash::Device,
    window_id: Option<usize>,
    width: u32,
    height: u32,
    bgra: bool,
    create: impl FnOnce(vk::DeviceSize) -> Result<(vk::Buffer, vk::DeviceMemory), String>,
) -> Option<Arc<CaptureBuffer>> {
    if !crate::screen_capture::capture_wants_surface(window_id) {
        if !crate::screen_capture::screen_capture_active() {
            RING.with(|ring| {
                if ring.borrow().as_ref().is_some_and(|r| r.pending.is_empty()) {
                    ring.borrow_mut().take();
                }
            });
        }
        return None;
    }
    if width < 2 || height < 2 {
        return None;
    }
    RING.with(|ring| {
        let mut ring = ring.borrow_mut();
        let key = (width, height, bgra);
        if ring.as_ref().is_some_and(|r| r.key != key) {
            // Buffers still held elsewhere stay alive until they are dropped.
            let pending = ring.take().map(|r| r.pending).unwrap_or_default();
            *ring = Some(Ring { key, buffers: Vec::new(), pending });
        }
        let ring = ring.get_or_insert_with(|| Ring { key, buffers: Vec::new(), pending: Vec::new() });
        if let Some(free) = ring.buffers.iter().find(|b| Arc::strong_count(b) == 1) {
            return Some(free.clone());
        }
        if ring.buffers.len() >= RING_BUFFERS {
            SKIPPED.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let stride = width as usize * 4;
        let len = stride * height as usize;
        let (buffer, memory) = match create(len as vk::DeviceSize) {
            Ok(created) => created,
            Err(err) => {
                crate::error!("screen capture: ring buffer {}x{}: {}", width, height, err);
                return None;
            }
        };
        let mapped = match unsafe {
            device.map_memory(memory, 0, len as vk::DeviceSize, vk::MemoryMapFlags::empty())
        } {
            Ok(mapped) => mapped as *const u8,
            Err(err) => {
                unsafe {
                    device.destroy_buffer(buffer, None);
                    device.free_memory(memory, None);
                }
                crate::error!("screen capture: map_memory: {:?}", err);
                return None;
            }
        };
        let captured = Arc::new(CaptureBuffer {
            device: device.clone(),
            buffer,
            memory,
            mapped,
            len,
            stride,
            bgra,
        });
        ring.buffers.push(captured.clone());
        Some(captured)
    })
}

/// The copy into `buffer` was recorded in the frame being submitted: hand it
/// to the sinks once that submission is known complete ([`deliver_completed`]).
/// `width` x `height` is what the sinks see (even-cropped).
pub(crate) fn queue(window_id: Option<usize>, buffer: Arc<CaptureBuffer>, width: u32, height: u32) {
    RING.with(|ring| {
        if let Some(ring) = ring.borrow_mut().as_mut() {
            ring.pending.push((window_id, buffer, width & !1, height & !1));
        }
    });
}

/// Every submission before the one about to be recorded has completed (its
/// fence was waited on): deliver what they copied.
pub(crate) fn deliver_completed() {
    let pending = RING.with(|ring| {
        ring.borrow_mut().as_mut().map(|r| std::mem::take(&mut r.pending)).unwrap_or_default()
    });
    if pending.is_empty() {
        return;
    }
    crate::trace!("screencap", "delivering {} (skipped so far: {})", pending.len(), SKIPPED.load(Ordering::Relaxed));
    for (window_id, buffer, width, height) in pending {
        crate::screen_capture::deliver_capture_surface(
            window_id,
            ScreenCaptureSurface {
                inner: SurfaceInner(buffer),
                width,
                height,
                time_ns: 0,
            },
        );
    }
}
