//! A texture as an encoder's GPU picture, the same on every OS: its shared surface handle and
//! the completion of the frame that drew it (`makepad_video::{SurfaceHandle, GpuCompletion}`).
use crate::{Cx, Texture};
use makepad_hal::completion as hal;
use makepad_video::{CompletionSource, GpuCompletion, SurfaceFormat, SurfaceHandle, SyncFile};
#[cfg(any(target_os = "linux", target_vendor = "apple", target_os = "windows"))]
use makepad_video::SurfaceKind;

/// A frame completion answered: the readers waiting on one look, and the UI (a reader polling
/// on the UI thread) wakes.
extern "C" fn completion_answered() {
    makepad_video::completion_notified();
    crate::thread::SignalToUI::set_ui_signal();
}

/// The HAL's frame completion `id` (`hal_frame_completion`).
struct HalCompletion(u64);

impl CompletionSource for HalCompletion {
    fn poll(&mut self) -> Option<Result<Option<SyncFile>, String>> {
        let mut fd = -1;
        let answer = match hal::hal_frame_completion_poll(self.0, &mut fd) {
            0 => return None,
            1 => Ok(sync_file(fd)),
            _ => Err(completion_error()),
        };
        self.0 = 0;
        Some(answer)
    }
}

impl Drop for HalCompletion {
    fn drop(&mut self) {
        if self.0 != 0 {
            hal::hal_frame_completion_drop(self.0);
        }
    }
}

#[cfg(target_os = "linux")]
fn sync_file(fd: i32) -> Option<SyncFile> {
    use std::os::fd::FromRawFd;
    (fd >= 0).then(|| unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
}

/// No sync_file off Linux (the HAL answers -1).
#[cfg(not(target_os = "linux"))]
fn sync_file(_fd: i32) -> Option<SyncFile> {
    None
}

fn completion_error() -> String {
    let mut text = vec![0u8; 256];
    let n = hal::hal_frame_completion_error(&mut text) as usize;
    if n > text.len() {
        text.resize(n, 0);
        hal::hal_frame_completion_error(&mut text);
    }
    text.truncate(n);
    String::from_utf8_lossy(&text).into_owned()
}

/// Linux: the DMA-BUF registration a surface handle holds, released with its last clone.
#[cfg(target_os = "linux")]
struct Registered(u64);

#[cfg(target_os = "linux")]
impl Drop for Registered {
    fn drop(&mut self) {
        makepad_hal::dmabuf::hal_dmabuf_release(self.0);
    }
}

impl Cx {
    /// The shared texture `texture` (a `SharedBgra8` render target, `create_shared_texture`)
    /// as an encoder's surface holding a `size` picture laid out as `format`. `None` until the
    /// HAL exported it (the next frame's texture follows). Keep the handle with the texture: one
    /// export per texture (Linux registers its DMA-BUF once).
    pub fn encode_surface(&mut self, texture: &Texture, size: [u32; 2], format: SurfaceFormat) -> Option<SurfaceHandle> {
        let handle = self.shared_texture_handle(texture).filter(|h| *h != 0)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::IntoRawFd;
            let _ = handle;
            let (fd, layout) = self.shared_texture_dmabuf(texture).map_err(|e| crate::log!("encode surface: {e}")).ok()?;
            let id = makepad_hal::dmabuf::hal_dmabuf_register(fd.into_raw_fd(), &layout);
            if id == 0 {
                crate::log!("encode surface: the DMA-BUF was refused");
                return None;
            }
            return Some(SurfaceHandle::new(id, SurfaceKind::DmaBuf, size, format).keeping(std::sync::Arc::new(Registered(id))));
        }
        #[cfg(target_vendor = "apple")]
        return Some(SurfaceHandle::new(handle, SurfaceKind::IoSurface, size, format));
        #[cfg(target_os = "windows")]
        return Some(SurfaceHandle::new(handle, SurfaceKind::D3d11Keyed, size, format));
        #[cfg(not(any(target_os = "linux", target_vendor = "apple", target_os = "windows")))]
        {
            let _ = (handle, size, format);
            return None;
        }
    }

    /// The completion of everything drawn so far (the batch is submitted first): when the
    /// pictures this frame drew may be read outside Cx. One per frame, cloned per reader.
    pub fn frame_completion(&mut self) -> Result<GpuCompletion, String> {
        self.hal_submit_batch();
        match hal::hal_frame_completion(self.hal.frames.w.session(), completion_answered) {
            0 => Err(completion_error()),
            id => Ok(GpuCompletion::from_source(Box::new(HalCompletion(id)))),
        }
    }
}
