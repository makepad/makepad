//! Screen-capture surfaces: a window's presented frames for a recorder,
//! without a CPU readback (the backend half of
//! [`crate::screen_capture::add_screen_capture_surface`]).
//!
//! While a surface sink is due a frame, the window pass takes the next free
//! IOSurface-backed BGRA `CVPixelBuffer` from a small pool, wraps it as a
//! Metal texture (CVMetalTextureCache) and blits its drawable into it, in the
//! same command buffer that presents the drawable. That command buffer's
//! completion hands the buffer to the sinks. One GPU copy per recorded frame;
//! nothing is read back, converted or waited for on the CPU. A video encoder
//! takes the buffer as is (VideoToolbox converts it on the media engine).
//!
//! The pool is capped: buffers are held by the GPU, by a sink's pending slot
//! and by the encoder until it has encoded them. When every one is taken the
//! frame is skipped, never waited for: a recorder that falls behind drops
//! frames instead of stalling the app.

use {
    crate::{
        makepad_objc_sys::objc_block,
        os::apple::apple_sys::*,
        screen_capture::{ScreenCaptureSurface, SurfaceInner},
    },
    makepad_objc_sys::{class, msg_send, sel, sel_impl},
    std::cell::RefCell,
    std::ffi::c_void,
    std::sync::atomic::{AtomicU64, Ordering},
    std::sync::Mutex,
};

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

/// Frames skipped because every capture buffer was in use. `MAKEPAD_TRACE=screencap`.
static SKIPPED: AtomicU64 = AtomicU64::new(0);

struct Pool {
    cache: CVMetalTextureCacheRef,
    device: ObjcId,
    pool: CVPixelBufferPoolRef,
    size: (usize, usize),
    /// `{kCVPixelBufferPoolAllocationThresholdKey: POOL_BUFFERS}`, retained.
    aux: ObjcId,
}

thread_local! {
    // Only the UI thread encodes passes; the pool lives as long as it.
    static POOL: RefCell<Option<Pool>> = const { RefCell::new(None) };
}

/// A pass's drawable is about to be presented: if a surface sink is due a
/// frame of `window_id`, blit `source` into a capture buffer on
/// `command_buffer` and deliver it when that buffer completes. Costs one
/// encoded blit; skips the frame when no buffer is free.
pub(crate) fn encode_capture_surface(
    device: ObjcId,
    command_buffer: ObjcId,
    source: ObjcId,
    window_id: Option<usize>,
) {
    if !crate::screen_capture::capture_wants_surface(window_id) {
        // Recording over: give the buffers back instead of pooling them.
        if !crate::screen_capture::screen_capture_active() {
            POOL.with(|pool| pool.borrow_mut().take());
        }
        return;
    }
    let (source_width, source_height, format): (usize, usize, u64) = unsafe {
        (msg_send![source, width], msg_send![source, height], msg_send![source, pixelFormat])
    };
    // Even dimensions for a 4:2:0 encoder: the odd last column/row is dropped.
    let (width, height) = (source_width & !1, source_height & !1);
    if width < 2 || height < 2 || format != MTLPixelFormat::BGRA8Unorm as u64 {
        return;
    }
    let started = std::time::Instant::now();
    let Some((pixel_buffer, cv_texture)) = POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        if pool.as_ref().is_some_and(|p| p.device != device || p.size != (width, height)) {
            *pool = None;
        }
        if pool.is_none() {
            *pool = Pool::new(device, width, height);
        }
        pool.as_mut()?.take()
    }) else {
        return;
    };
    unsafe {
        let destination = CVMetalTextureGetTexture(cv_texture);
        let blit: ObjcId = msg_send![command_buffer, blitCommandEncoder];
        let () = msg_send![
            blit,
            copyFromTexture: source
            sourceSlice: 0u64
            sourceLevel: 0u64
            sourceOrigin: MTLOrigin { x: 0, y: 0, z: 0 }
            sourceSize: MTLSize { width: width as u64, height: height as u64, depth: 1 }
            toTexture: destination
            destinationSlice: 0u64
            destinationLevel: 0u64
            destinationOrigin: MTLOrigin { x: 0, y: 0, z: 0 }
        ];
        let () = msg_send![blit, endEncoding];
    }
    // The Metal texture must outlive the GPU's write; the surface goes to
    // the sinks once the write is done. Taken once: a block may be copied.
    let in_flight = Mutex::new(Some((
        SendPtr(cv_texture),
        ScreenCaptureSurface {
            inner: SurfaceInner(pixel_buffer),
            width: width as u32,
            height: height as u32,
            time_ns: 0,
        },
    )));
    let () = unsafe {
        msg_send![
            command_buffer,
            addCompletedHandler: &objc_block!(move |_command_buffer: ObjcId| {
                if let Some((cv_texture, surface)) = in_flight.lock().unwrap().take() {
                    unsafe { CFRelease(cv_texture.0 as *const c_void) };
                    crate::screen_capture::deliver_capture_surface(window_id, surface);
                }
            })
        ]
    };
    crate::trace!(
        "screencap",
        "surface {}x{} encoded in {:.3} ms (skipped so far: {})",
        width,
        height,
        started.elapsed().as_secs_f64() * 1000.0,
        SKIPPED.load(Ordering::Relaxed)
    );
}

struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}

impl Pool {
    fn new(device: ObjcId, width: usize, height: usize) -> Option<Self> {
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
                crate::error!("screen capture: CVMetalTextureCacheCreate failed ({})", status);
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
                crate::error!("screen capture: CVPixelBufferPoolCreate {}x{} failed ({})", width, height, status);
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
    fn take(&mut self) -> Option<(CVPixelBufferRef, CVMetalTextureRef)> {
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
                crate::error!("screen capture: no pixel buffer ({})", status);
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
                crate::error!("screen capture: no Metal texture for the pixel buffer ({})", status);
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
