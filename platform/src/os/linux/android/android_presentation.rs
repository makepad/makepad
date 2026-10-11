//! Timestamp Choreographer UI buffers and optionally trace their actual presentation.
use super::super::egl_sys::{EGLDisplay, EGLSurface, LibEgl};
use std::{collections::VecDeque, ffi::CStr};

const EGL_EXTENSIONS: i32 = 0x3055;
const EGL_TIMESTAMPS_ANDROID: i32 = 0x3430;
const EGL_TIMESTAMP_PENDING_ANDROID: i64 = -2;
const EGL_COMPOSITE_INTERVAL_ANDROID: i32 = 0x3432;
// Requested present, rendering complete, composition latch, display present.
const TRACE_NAMES: [i32; 4] = [0x3434, 0x3435, 0x3436, 0x343a];

type NextId = unsafe extern "C" fn(EGLDisplay, EGLSurface, *mut u64) -> u32;
type Timestamps =
    unsafe extern "C" fn(EGLDisplay, EGLSurface, u64, i32, *const i32, *mut i64) -> u32;
type Timing = unsafe extern "C" fn(EGLDisplay, EGLSurface, i32, *const i32, *mut i64) -> u32;
type Supported = unsafe extern "C" fn(EGLDisplay, EGLSurface, i32) -> u32;

#[repr(C)]
struct Timespec {
    seconds: std::os::raw::c_long,
    nanos: std::os::raw::c_long,
}
unsafe extern "C" {
    fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
}

#[derive(Default)]
pub(super) struct AndroidPresentation {
    initialized: bool,
    presentation_supported: Option<bool>,
    compositor_timing: Option<Timing>,
    functions: Option<(NextId, Timestamps)>,
    trace_supported: bool,
    next_swap: Option<(u64, Option<u64>)>,
    pending_trace: VecDeque<(u64, u64)>,
}

impl AndroidPresentation {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    unsafe fn initialize_trace(&mut self, egl: &LibEgl, display: EGLDisplay, surface: EGLSurface) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        let extensions = (egl.eglQueryString.unwrap())(display, EGL_EXTENSIONS);
        if extensions.is_null()
            || !CStr::from_ptr(extensions)
                .to_bytes()
                .split(|b| *b == b' ')
                .any(|s| s == b"EGL_ANDROID_get_frame_timestamps")
        {
            return;
        }
        let next = (egl.eglGetProcAddress.unwrap())(c"eglGetNextFrameIdANDROID".as_ptr());
        let timestamps = (egl.eglGetProcAddress.unwrap())(c"eglGetFrameTimestampsANDROID".as_ptr());
        let supported =
            (egl.eglGetProcAddress.unwrap())(c"eglGetFrameTimestampSupportedANDROID".as_ptr());
        if next.is_null() || timestamps.is_null() || supported.is_null() {
            return;
        }
        let supported: Supported = std::mem::transmute(supported);
        if !TRACE_NAMES[..3]
            .iter()
            .all(|name| supported(display, surface, *name) != 0)
        {
            return;
        }
        if (egl.eglSurfaceAttrib.unwrap())(display, surface, EGL_TIMESTAMPS_ANDROID, 1) == 0 {
            return;
        }
        self.trace_supported = TRACE_NAMES
            .iter()
            .all(|name| supported(display, surface, *name) != 0);
        self.functions = Some((std::mem::transmute(next), std::mem::transmute(timestamps)));
    }

    pub(super) unsafe fn before_swap(
        &mut self,
        egl: &LibEgl,
        display: EGLDisplay,
        surface: EGLSurface,
    ) {
        if self.presentation_supported.is_none() {
            self.presentation_supported = Some(false);
            let extensions = (egl.eglQueryString.unwrap())(display, EGL_EXTENSIONS);
            if egl.eglPresentationTimeANDROID.is_some() && !extensions.is_null() {
                let extensions = CStr::from_ptr(extensions).to_bytes();
                let has = |name: &[u8]| extensions.split(|b| *b == b' ').any(|s| s == name);
                if has(b"EGL_ANDROID_presentation_time") && has(b"EGL_ANDROID_get_frame_timestamps")
                {
                    let timing =
                        (egl.eglGetProcAddress.unwrap())(c"eglGetCompositorTimingANDROID".as_ptr());
                    let supported = (egl.eglGetProcAddress.unwrap())(
                        c"eglGetCompositorTimingSupportedANDROID".as_ptr(),
                    );
                    if !timing.is_null() && !supported.is_null() {
                        let supported: Supported = std::mem::transmute(supported);
                        if supported(display, surface, EGL_COMPOSITE_INTERVAL_ANDROID) != 0 {
                            self.compositor_timing = Some(std::mem::transmute(timing));
                            self.presentation_supported = Some(true);
                        }
                    }
                }
            }
        }
        if self.presentation_supported == Some(true) {
            let mut interval = 0i64;
            let mut time: Timespec = std::mem::zeroed();
            let timing = self.compositor_timing.unwrap();
            if timing(
                display,
                surface,
                1,
                &EGL_COMPOSITE_INTERVAL_ANDROID,
                &mut interval,
            ) != 0
                && (1_000_000..=100_000_000).contains(&interval)
                && clock_gettime(1, &mut time) == 0
            {
                // Give GPU work a refresh to finish. Explicit timestamps let
                // Android retire older queued UI states without choosing an
                // unfinished replacement for the imminent display refresh.
                let ns = time.seconds as i64 * 1_000_000_000 + time.nanos as i64 + interval;
                if (egl.eglPresentationTimeANDROID.unwrap())(display, surface, ns) == 0 {
                    self.presentation_supported = Some(false);
                }
            }
        }
        if !crate::makepad_error_log::trace_enabled("android.present") {
            self.pending_trace.clear();
            return;
        }
        self.initialize_trace(egl, display, surface);
        let Some((next, timestamps)) = self.functions else {
            return;
        };
        let trace =
            self.trace_supported && crate::makepad_error_log::trace_enabled("android.present");
        if trace {
            // Match actual displayed buffers by id, never by guessed queue
            // timestamps. Poll without waiting; bound history on bad drivers.
            self.pending_trace.retain(|&(id, submitted_ns)| {
                let mut values = [0i64; 4];
                if timestamps(display, surface, id, 4, TRACE_NAMES.as_ptr(), values.as_mut_ptr()) == 0 { return false; }
                if values[3] == EGL_TIMESTAMP_PENDING_ANDROID { return true; }
                crate::log!("[android.present] id={} submitted_ns={} requested_ns={} ready_ns={} latch_ns={} present_ns={}",
                    id, submitted_ns, values[0], values[1], values[2], values[3]);
                false
            });
            if self.pending_trace.len() >= 64 {
                self.pending_trace.pop_front();
            }
        } else {
            self.pending_trace.clear();
        }
        let mut id = 0;
        if next(display, surface, &mut id) != 0 {
            let mut time: Timespec = std::mem::zeroed();
            let ns = (trace && clock_gettime(1, &mut time) == 0)
                .then(|| time.seconds as u64 * 1_000_000_000 + time.nanos as u64);
            self.next_swap = Some((id, ns));
        }
    }

    pub(super) fn after_swap(&mut self, success: bool) {
        if let Some((id, ns)) = self.next_swap.take() {
            if success {
                if let Some(ns) = ns {
                    self.pending_trace.push_back((id, ns));
                }
            }
        }
    }
}
