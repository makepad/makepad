//! Portable media frame descriptors and serialized D3D media access.
//! Native GPU texture storage and imports are owned by the HAL.

use crate::{texture::{Texture, TextureFormat}, Cx};
use std::{
    cell::{Cell, RefCell},
    sync::{Mutex, MutexGuard},
};

/// Serializes hard-decode / media GPU work with Makepad present copies on the
/// shared D3D11 device. Recursive so the same thread may nest lock calls
/// (common when a decoder callback re-enters present).
static MEDIA_D3D11_MUTEX: Mutex<()> = Mutex::new(());

thread_local! {
    static MEDIA_D3D11_LOCK_DEPTH: Cell<u32> = const { Cell::new(0) };
    static MEDIA_D3D11_LOCK_GUARD: RefCell<Option<MutexGuard<'static, ()>>> =
        const { RefCell::new(None) };
}

/// Acquire the shared media D3D11 lock (recursive on the same thread).
pub fn media_d3d11_lock() {
    MEDIA_D3D11_LOCK_DEPTH.with(|depth| {
        if depth.get() == 0 {
            let guard = MEDIA_D3D11_MUTEX.lock().unwrap();
            // Mutex is 'static; hold the guard across paired lock/unlock calls.
            let guard: MutexGuard<'static, ()> = unsafe { std::mem::transmute(guard) };
            MEDIA_D3D11_LOCK_GUARD.with(|slot| {
                *slot.borrow_mut() = Some(guard);
            });
        }
        depth.set(depth.get().saturating_add(1));
    });
}

/// Release the shared media D3D11 lock.
pub fn media_d3d11_unlock() {
    MEDIA_D3D11_LOCK_DEPTH.with(|depth| {
        let next = depth.get().saturating_sub(1);
        depth.set(next);
        if next == 0 {
            MEDIA_D3D11_LOCK_GUARD.with(|slot| {
                drop(slot.borrow_mut().take());
            });
        }
    });
}

/// Run `f` while holding the shared media D3D11 lock.
pub fn with_media_d3d11_lock<R>(f: impl FnOnce() -> R) -> R {
    media_d3d11_lock();
    let out = f();
    media_d3d11_unlock();
    out
}

/// C ABI wrapper around [`media_d3d11_lock`] for native decoder device-context
/// lock callbacks (function-pointer slots).
pub unsafe extern "C" fn media_d3d11_c_lock(_lock_ctx: *mut std::ffi::c_void) {
    media_d3d11_lock();
}

/// C ABI wrapper around [`media_d3d11_unlock`].
pub unsafe extern "C" fn media_d3d11_c_unlock(_lock_ctx: *mut std::ffi::c_void) {
    media_d3d11_unlock();
}

impl Texture {
    /// Allocate a [`TextureFormat::VideoExternal`] slot for later `adopt_*`.
    #[track_caller]
    pub fn new_video_external(cx: &mut Cx) -> Self {
        Texture::new_with_format(cx, TextureFormat::VideoExternal)
    }

    /// Allocate a [`TextureFormat::VideoYuvPlane`] slot for Y / U / V (or NV12 UV).
    #[track_caller]
    pub fn new_video_yuv_plane(cx: &mut Cx) -> Self {
        Texture::new_with_format(cx, TextureFormat::VideoYuvPlane)
    }
}

/// Zero-copy OES frame for Android `sample_video` present.
#[cfg(target_os = "android")]
pub struct OesFrame {
    pub tex_id: u32,
    pub width: u32,
    pub height: u32,
    pub keep_alive: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}
