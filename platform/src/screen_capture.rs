//! Continuous window readback: every presented frame of a window, as raw
//! RGBA, on the GPU completion thread.
//!
//! The one-shot cousins of this ride the same wire: `--remote` `/g` grabs,
//! studio screenshots, `Cx::capture_next_frame_to_file` and
//! [`crate::pixel_probe`] all queue a *request* that one pass answers and
//! then forget it. A capture sink is instead standing permission: while one
//! is installed for a window, that window's pass blits its drawable into a
//! shared texture every frame and hands the bytes here — which is what a
//! screen recorder needs and a request queue cannot express.
//!
//! Sinks run on the backend's frame-completion thread (Metal's
//! `addCompletedHandler`, the equivalent elsewhere). Copy the bytes and get
//! off it; do not block, and do not call `add_screen_capture` /
//! `remove_screen_capture` from inside a sink (the registry lock is held).
//!
//! Backend coverage: macOS/Metal, Windows/D3D11, Linux/OpenGL + Vulkan —
//! the same places that can already answer a `/g` grab.
//!
//! A recorder uses the second kind of sink, [`add_screen_capture_surface`],
//! where the backend supports it ([`surfaces_supported`]: Metal, D3D11, the
//! windowed Vulkan path). It gets the frame as a [`ScreenCaptureSurface`]
//! instead of bytes: the window pass copies its drawable on the GPU into the
//! next free buffer of a small ring, in the work that presents it, and the
//! render thread never waits for or touches the pixels. A frame is skipped
//! rather than waited for when every buffer is still in use. The backend
//! halves are `os/apple/capture_surface.rs`, `os/windows/capture_surface.rs`
//! and `os/linux/capture_surface.rs`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub struct ScreenCaptureFrame<'a> {
    /// The window the presenting pass belongs to, when the backend knows it.
    pub window_id: Option<usize>,
    /// Device pixels, not layout points.
    pub width: u32,
    pub height: u32,
    /// Tightly packed top-down RGBA8, `width * height * 4` bytes.
    pub rgba: &'a [u8],
    /// Monotonic nanoseconds since the first capture-carrying frame of the
    /// process. The recorder's frame clock — wall time, not a frame counter,
    /// so a dropped or duplicated present lands at the time it happened.
    pub time_ns: u64,
}

pub type ScreenCaptureFn = Box<dyn FnMut(&ScreenCaptureFrame) + Send + 'static>;

#[derive(Clone, Copy, Debug)]
pub struct ScreenCaptureOptions {
    /// Capture only this window's passes. `None` takes whichever window
    /// presents — right for a single-window app, wrong for a recorder that
    /// means one specific window.
    pub window_id: Option<usize>,
    /// Upper bound on delivered frames per second. The readback is a full
    /// drawable blit plus a CPU copy, so a 120Hz window recorded at 30 does
    /// a quarter of the work. `0.0` = every presented frame.
    pub max_fps: f64,
}

impl Default for ScreenCaptureOptions {
    fn default() -> Self {
        Self {
            window_id: None,
            max_fps: 0.0,
        }
    }
}

/// A presented frame as a GPU-side surface, `width` x `height` device pixels
/// (the drawable cropped to even dimensions, which is what a 4:2:0 encoder
/// wants), top-down. What it holds is per backend:
///
/// - Apple: an IOSurface-backed BGRA8 `CVPixelBuffer` ([`Self::pixel_buffer`]).
/// - Windows: a BGRA8 `ID3D11Texture2D` on the app's device
///   ([`Self::d3d11_texture`]), which Media Foundation encodes from directly.
/// - Linux (Vulkan): a persistently mapped host-visible buffer the frame's
///   command buffer copied into; the bytes are read by the consumer's thread.
///
/// Cloning shares the surface; dropping the last handle gives the buffer back
/// to the backend's small capture ring, so a sink that holds on to surfaces
/// holds capture buffers (hold the newest only). With none free the backend
/// skips frames rather than waiting.
#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
#[derive(Clone)]
pub struct ScreenCaptureSurface {
    pub(crate) inner: SurfaceInner,
    pub width: u32,
    pub height: u32,
    /// Same clock as [`ScreenCaptureFrame::time_ns`].
    pub time_ns: u64,
}

#[cfg(target_vendor = "apple")]
pub(crate) struct SurfaceInner(pub(crate) crate::os::apple::apple_sys::CVPixelBufferRef);

// A CVPixelBuffer is a thread-safe CF object; the handle only retains it.
#[cfg(target_vendor = "apple")]
unsafe impl Send for SurfaceInner {}
#[cfg(target_vendor = "apple")]
unsafe impl Sync for SurfaceInner {}

#[cfg(target_vendor = "apple")]
impl Clone for SurfaceInner {
    fn clone(&self) -> Self {
        unsafe { crate::os::apple::apple_sys::CVPixelBufferRetain(self.0) };
        Self(self.0)
    }
}

#[cfg(target_vendor = "apple")]
impl Drop for SurfaceInner {
    fn drop(&mut self) {
        unsafe { crate::os::apple::apple_sys::CVPixelBufferRelease(self.0) };
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone)]
pub(crate) struct SurfaceInner {
    pub(crate) texture: crate::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    pub(crate) device: crate::windows::Win32::Graphics::Direct3D11::ID3D11Device,
}

#[cfg(target_os = "linux")]
#[derive(Clone)]
pub(crate) struct SurfaceInner(pub(crate) std::sync::Arc<dyn MappedCapture>);

/// A Linux capture buffer, mapped for reading while any handle lives.
#[cfg(target_os = "linux")]
pub(crate) trait MappedCapture: Send + Sync {
    /// Rows of `stride()` bytes, top row first.
    fn bytes(&self) -> &[u8];
    fn stride(&self) -> usize;
    /// BGRA (true) or RGBA (false).
    fn bgra(&self) -> bool;
}

#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
impl ScreenCaptureSurface {
    /// The `CVPixelBufferRef`, valid while `self` lives.
    #[cfg(target_vendor = "apple")]
    pub fn pixel_buffer(&self) -> crate::os::apple::apple_sys::CVPixelBufferRef {
        self.inner.0
    }

    /// The texture holding the frame (BGRA8, exactly `width` x `height`).
    #[cfg(target_os = "windows")]
    pub fn d3d11_texture(&self) -> &crate::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D {
        &self.inner.texture
    }

    /// The device the texture lives on, for an encoder that reads it there.
    #[cfg(target_os = "windows")]
    pub fn d3d11_device(&self) -> &crate::windows::Win32::Graphics::Direct3D11::ID3D11Device {
        &self.inner.device
    }

    /// Copy the pixels out as tightly packed opaque RGBA8, on the caller's
    /// thread. On Apple and Linux the surface is CPU-visible memory (the GPU
    /// finished writing it before it was delivered); on Windows this is a
    /// staging copy on the device and waits for it, so keep it off the UI
    /// thread and out of the per-frame path.
    pub fn read_rgba(&self, out: &mut Vec<u8>) -> bool {
        let (width, height) = (self.width as usize, self.height as usize);
        out.clear();
        out.resize(width * height * 4, 0);
        #[cfg(target_vendor = "apple")]
        unsafe {
            use crate::os::apple::apple_sys::*;
            // kCVPixelBufferLock_ReadOnly
            if CVPixelBufferLockBaseAddress(self.inner.0, 1) != 0 {
                return false;
            }
            let base = CVPixelBufferGetBaseAddress(self.inner.0) as *const u8;
            let stride = CVPixelBufferGetBytesPerRow(self.inner.0);
            let ok = !base.is_null() && stride >= width * 4;
            if ok {
                for (y, row) in out.chunks_exact_mut(width * 4).enumerate() {
                    let src = std::slice::from_raw_parts(base.add(y * stride), width * 4);
                    swizzle_opaque(row, src, true);
                }
            }
            CVPixelBufferUnlockBaseAddress(self.inner.0, 1);
            ok
        }
        #[cfg(target_os = "windows")]
        {
            crate::os::windows::capture_surface::read_texture_rgba(self, out)
        }
        #[cfg(target_os = "linux")]
        {
            let (bytes, stride) = (self.inner.0.bytes(), self.inner.0.stride());
            if stride < width * 4 || bytes.len() < stride * height {
                return false;
            }
            for (y, row) in out.chunks_exact_mut(width * 4).enumerate() {
                swizzle_opaque(row, &bytes[y * stride..y * stride + width * 4], self.inner.0.bgra());
            }
            true
        }
    }
}

/// `src` pixels to opaque RGBA in `dst`, swapping red and blue when `bgra`.
#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
pub(crate) fn swizzle_opaque(dst: &mut [u8], src: &[u8], bgra: bool) {
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        if bgra {
            d.copy_from_slice(&[s[2], s[1], s[0], 255]);
        } else {
            d.copy_from_slice(&[s[0], s[1], s[2], 255]);
        }
    }
}

#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
pub type ScreenCaptureSurfaceFn = Box<dyn FnMut(&ScreenCaptureSurface) + Send + 'static>;

/// Set by a backend that delivers surfaces ([`add_screen_capture_surface`]);
/// a recorder falls back to byte frames where none does (a GL window, the
/// DRM direct path).
static SURFACES_SUPPORTED: AtomicBool = AtomicBool::new(false);

/// Whether this process's renderer delivers capture surfaces.
pub fn surfaces_supported() -> bool {
    // Metal and D3D11 are the only renderers there; Linux has several.
    cfg!(any(target_vendor = "apple", target_os = "windows"))
        || SURFACES_SUPPORTED.load(Ordering::Acquire)
}

#[allow(dead_code)] // set by the backends that deliver surfaces
pub(crate) fn set_surfaces_supported() {
    SURFACES_SUPPORTED.store(true, Ordering::Release);
}

enum SinkFn {
    Bytes(ScreenCaptureFn),
    #[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
    Surface(ScreenCaptureSurfaceFn),
}

struct Sink {
    window_id: Option<usize>,
    min_interval_ns: u64,
    last_ns: Option<u64>,
    f: SinkFn,
}

impl Sink {
    fn is_bytes(&self) -> bool {
        matches!(self.f, SinkFn::Bytes(_))
    }
    fn matches(&self, window_id: Option<usize>) -> bool {
        match self.window_id {
            None => true,
            Some(want) => window_id == Some(want),
        }
    }
    /// Is this sink owed a frame yet?
    ///
    /// The gate is 3/4 of the interval, not the whole of it. A recorder asking
    /// for exactly the display's refresh gets presents spaced one refresh
    /// apart *give or take jitter*, and a strict `>=` would reject the ones
    /// that land a hair early and take the NEXT one instead — turning a
    /// 60-on-60Hz request into a ragged 30. Three quarters accepts every
    /// present at the asked-for rate while still halving a 120Hz window down
    /// to a 60fps recording.
    fn due(&self, now_ns: u64) -> bool {
        match self.last_ns {
            None => true,
            Some(last) => now_ns.saturating_sub(last) >= self.min_interval_ns / 4 * 3,
        }
    }
}

/// Read before the registry lock so a window with no recorder attached pays
/// one relaxed atomic load per pass.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn sinks() -> &'static Mutex<HashMap<u64, Sink>> {
    static SINKS: OnceLock<Mutex<HashMap<u64, Sink>>> = OnceLock::new();
    SINKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ns() -> u64 {
    (crate::Cx::monotonic_now() * 1_000_000_000.0) as u64
}

pub fn add_screen_capture<F>(options: ScreenCaptureOptions, f: F) -> u64
where
    F: FnMut(&ScreenCaptureFrame) + Send + 'static,
{
    add_screen_capture_box(options, Box::new(f))
}

pub fn add_screen_capture_box(options: ScreenCaptureOptions, f: ScreenCaptureFn) -> u64 {
    add_sink(options, SinkFn::Bytes(f))
}

/// Install a surface sink (see the module docs): every due presented frame of
/// the window, as a GPU surface, on the backend's frame-completion thread.
#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
pub fn add_screen_capture_surface<F>(options: ScreenCaptureOptions, f: F) -> u64
where
    F: FnMut(&ScreenCaptureSurface) + Send + 'static,
{
    add_sink(options, SinkFn::Surface(Box::new(f)))
}

fn add_sink(options: ScreenCaptureOptions, f: SinkFn) -> u64 {
    let min_interval_ns = if options.max_fps > 0.0 {
        (1_000_000_000.0 / options.max_fps) as u64
    } else {
        0
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut sinks = sinks().lock().unwrap();
    sinks.insert(
        id,
        Sink {
            window_id: options.window_id,
            min_interval_ns,
            last_ns: None,
            f,
        },
    );
    ACTIVE.store(true, Ordering::Release);
    id
}

pub fn remove_screen_capture(id: u64) {
    let mut sinks = sinks().lock().unwrap();
    sinks.remove(&id);
    ACTIVE.store(!sinks.is_empty(), Ordering::Release);
}

/// True while at least one sink is installed, for any window.
pub fn screen_capture_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Should this pass pay for a readback? Called on the render thread while
/// encoding the pass, i.e. one frame BEFORE `deliver_capture_frame` runs.
pub fn capture_wants_window(window_id: Option<usize>) -> bool {
    if !ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let Ok(sinks) = sinks().lock() else {
        return false;
    };
    let now = now_ns();
    sinks
        .values()
        .any(|sink| sink.is_bytes() && sink.matches(window_id) && sink.due(now))
}

/// Should this pass blit its drawable into a capture surface?
#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
pub fn capture_wants_surface(window_id: Option<usize>) -> bool {
    if !ACTIVE.load(Ordering::Acquire) {
        return false;
    }
    let Ok(sinks) = sinks().lock() else {
        return false;
    };
    let now = now_ns();
    sinks
        .values()
        .any(|sink| !sink.is_bytes() && sink.matches(window_id) && sink.due(now))
}

/// Hand a captured surface to every surface sink that wants it. Called from
/// the backend's frame-completion path; `surface.time_ns` is set here.
#[cfg(any(target_vendor = "apple", target_os = "windows", target_os = "linux"))]
pub(crate) fn deliver_capture_surface(window_id: Option<usize>, mut surface: ScreenCaptureSurface) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let Ok(mut sinks) = sinks().lock() else {
        return;
    };
    let now = now_ns();
    surface.time_ns = now;
    for sink in sinks.values_mut() {
        if !sink.matches(window_id) || !sink.due(now) {
            continue;
        }
        if let SinkFn::Surface(f) = &mut sink.f {
            sink.last_ns = Some(now);
            f(&surface);
        }
    }
}

/// Hand the presented frame's RGBA to every sink that wants it. Called from
/// the backend's frame-completion path.
pub fn deliver_capture_frame(window_id: Option<usize>, width: u32, height: u32, rgba: &[u8]) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
        return;
    }
    let Ok(mut sinks) = sinks().lock() else {
        return;
    };
    let now = now_ns();
    let frame = ScreenCaptureFrame {
        window_id,
        width,
        height,
        rgba,
        time_ns: now,
    };
    for sink in sinks.values_mut() {
        if !sink.matches(window_id) || !sink.due(now) {
            continue;
        }
        // Irrefutable where the platform has no surface sinks.
        #[allow(irrefutable_let_patterns)]
        if let SinkFn::Bytes(f) = &mut sink.f {
            sink.last_ns = Some(now);
            f(&frame);
        }
    }
}
