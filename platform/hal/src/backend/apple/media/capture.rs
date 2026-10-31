//! Screen-capture surfaces: a window's presented frames for a recorder, without a CPU readback.
//!
//! While the app marks a window frame for capture (`target::CAPTURE`), the frame takes the next
//! free IOSurface-backed BGRA `CVPixelBuffer` from a small pool, wraps it as a Metal texture
//! (CVMetalTextureCache) and blits its drawable into it, in the same command buffer that
//! presents the drawable. That command buffer's completion hands the buffer to the app (a
//! `ScreenSurface` event). One GPU copy per recorded frame; nothing is read back, converted or
//! waited for on the CPU. A video encoder takes the buffer as is (VideoToolbox converts it on
//! the media engine).
//!
//! The pool is capped: buffers are held by the GPU, by a sink's pending slot and by the encoder
//! until it has encoded them. When every one is taken the frame is skipped, never waited for: a
//! recorder that falls behind drops frames instead of stalling the app.
//!
//! Moved from os/apple/capture_surface.rs; whether a frame is due is the app's call (it marks
//! the frame), the pool is the HAL's.

use makepad_apple_sys::*;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

type CVPixelBufferPoolRef = *mut c_void;

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVPixelBufferPoolAllocationThresholdKey: CFStringRef;
    fn CVPixelBufferPoolCreate(
        allocator: *const c_void,
        pool_attributes: *const c_void,
        pixel_buffer_attributes: *const c_void,
        pool_out: *mut CVPixelBufferPoolRef,
    ) -> CVReturn;
    fn CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
        allocator: *const c_void,
        pool: CVPixelBufferPoolRef,
        aux_attributes: *const c_void,
        pixel_buffer_out: *mut CVPixelBufferRef,
    ) -> CVReturn;
    fn CVPixelBufferPoolRelease(pool: CVPixelBufferPoolRef);
}

/// `kCVReturnWouldExceedAllocationThreshold`: every pooled buffer is in use.
const WOULD_EXCEED_ALLOCATION_THRESHOLD: CVReturn = -6689;

/// Buffers in use at once: two frames on the GPU, one in the sink's pending
/// slot, one the encoder repeats, and the few VideoToolbox keeps while it
/// encodes (it retains its input until the frame is out).
const POOL_BUFFERS: u32 = 16;

/// Frames skipped because every capture buffer was in use.
static SKIPPED: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Pool {
    cache: CVMetalTextureCacheRef,
    pub(crate) device: ObjcId,
    pool: CVPixelBufferPoolRef,
    pub(crate) size: (usize, usize),
    /// `{kCVPixelBufferPoolAllocationThresholdKey: POOL_BUFFERS}`, retained.
    aux: ObjcId,
}

// SAFETY: CoreVideo pools and caches are thread-safe objects
unsafe impl Send for Pool {}

impl Pool {
    pub(crate) fn new(device: ObjcId, width: usize, height: usize) -> Option<Self> {
        unsafe {
            let mut cache: CVMetalTextureCacheRef = std::ptr::null_mut();
            let status = CVMetalTextureCacheCreate(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                device,
                std::ptr::null_mut(),
                &mut cache,
            );
            if status != 0 || cache.is_null() {
                eprintln!("screen capture: CVMetalTextureCacheCreate failed ({})", status);
                return None;
            }
            let number = |value: u64| -> ObjcId { msg_send![class!(NSNumber), numberWithUnsignedLongLong: value] };
            let attributes: ObjcId = msg_send![class!(NSMutableDictionary), new];
            let entries: [(CFStringRef, ObjcId); 5] = [
                (kCVPixelBufferPixelFormatTypeKey, number(kCVPixelFormatType_32BGRA as u64)),
                (kCVPixelBufferWidthKey, number(width as u64)),
                (kCVPixelBufferHeightKey, number(height as u64)),
                (kCVPixelBufferMetalCompatibilityKey, msg_send![class!(NSNumber), numberWithBool: YES]),
                (kCVPixelBufferIOSurfacePropertiesKey, msg_send![class!(NSDictionary), dictionary]),
            ];
            for (key, value) in entries {
                let () = msg_send![attributes, setObject: value forKey: key as ObjcId];
            }
            let mut pool: CVPixelBufferPoolRef = std::ptr::null_mut();
            let status = CVPixelBufferPoolCreate(
                std::ptr::null(),
                std::ptr::null(),
                attributes as *const c_void,
                &mut pool,
            );
            let () = msg_send![attributes, release];
            if status != 0 || pool.is_null() {
                CFRelease(cache as *const c_void);
                eprintln!("screen capture: CVPixelBufferPoolCreate {}x{} failed ({})", width, height, status);
                return None;
            }
            let aux: ObjcId = msg_send![class!(NSMutableDictionary), new];
            let () = msg_send![
                aux,
                setObject: number(POOL_BUFFERS as u64)
                forKey: kCVPixelBufferPoolAllocationThresholdKey as ObjcId
            ];
            Some(Self { cache, device, pool, size: (width, height), aux })
        }
    }

    /// The next free buffer and its Metal texture, both +1; `None` when all
    /// are in use.
    pub(crate) fn take(&mut self) -> Option<(CVPixelBufferRef, CVMetalTextureRef)> {
        let (width, height) = self.size;
        unsafe {
            let mut pixel_buffer: CVPixelBufferRef = std::ptr::null_mut();
            let status = CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
                std::ptr::null(),
                self.pool,
                self.aux as *const c_void,
                &mut pixel_buffer,
            );
            if status == WOULD_EXCEED_ALLOCATION_THRESHOLD {
                SKIPPED.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            if status != 0 || pixel_buffer.is_null() {
                eprintln!("screen capture: no pixel buffer ({})", status);
                return None;
            }
            let mut cv_texture: CVMetalTextureRef = std::ptr::null_mut();
            let status = CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null_mut(),
                self.cache,
                pixel_buffer,
                std::ptr::null_mut(),
                MTLPixelFormat::BGRA8Unorm as u64,
                width,
                height,
                0,
                &mut cv_texture,
            );
            if status != 0 || cv_texture.is_null() {
                CVPixelBufferRelease(pixel_buffer);
                eprintln!("screen capture: no Metal texture for the pixel buffer ({})", status);
                return None;
            }
            Some((pixel_buffer, cv_texture))
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe {
            // Buffers still out (a sink's slot, the encoder) keep their own
            // references; the pool itself goes when they do.
            CVPixelBufferPoolRelease(self.pool);
            CVMetalTextureCacheFlush(self.cache, 0);
            CFRelease(self.cache as *const c_void);
            let () = msg_send![self.aux, release];
        }
    }
}
